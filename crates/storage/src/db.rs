use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use postgres::{
    Config,
    config::{Host, SslMode},
};
use postgres_native_tls::MakeTlsConnector;

use crate::clock::{Clock, SystemClock};
use crate::error::{StorageError, StorageResult};
use crate::migrate::{self, latest_version};
use crate::pg::{Connection, Manager, Pool};

const DATA_UPDATED_CACHE_TTL: Duration = Duration::from_secs(30);
const FEED_SNAPSHOT_INTERVAL_MS: i64 = 5 * 60 * 1_000;
// Published by Supabase Studio's custom-content configuration at
// https://supabase-downloads.s3-ap-southeast-1.amazonaws.com/prod/ssl/prod-ca-2021.crt
// Supabase Root 2021 CA expires on 2031-04-26.
const SUPABASE_ROOT_CA: &[u8] = include_bytes!("../certs/supabase-prod-ca-2021.crt");

fn is_supabase_database_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.strip_suffix(".pooler.supabase.com")
        .is_some_and(|prefix| !prefix.is_empty())
        || host
            .strip_prefix("db.")
            .and_then(|host| host.strip_suffix(".supabase.co"))
            .is_some_and(|project| !project.is_empty() && !project.contains('.'))
}

fn postgres_tls_connector(config: &Config) -> StorageResult<native_tls::TlsConnector> {
    let mut builder = native_tls::TlsConnector::builder();
    if config.get_hosts().iter().any(|host| match host {
        Host::Tcp(host) => is_supabase_database_host(host),
        #[cfg(unix)]
        Host::Unix(_) => false,
    }) {
        let certificate = native_tls::Certificate::from_pem(SUPABASE_ROOT_CA).map_err(|error| {
            StorageError::migration(format!(
                "cannot parse the bundled Supabase root CA: {error}"
            ))
        })?;
        // Add the official root alongside system roots. The connector still
        // validates the chain and the configured PostgreSQL hostname.
        builder.add_root_certificate(certificate);
    }
    builder.build().map_err(|error| {
        StorageError::migration(format!("cannot initialize PostgreSQL TLS: {error}"))
    })
}

/// Bounded PostgreSQL pool. Use Supabase's session pooler or direct connection;
/// connection state and prepared statements require a session, not transaction pooling.
#[derive(Clone)]
pub struct Database {
    pool: Arc<ManagedPool>,
    database_url: Arc<str>,
    schema: Arc<str>,
    clock: Arc<dyn Clock>,
    test_schema: Option<Arc<TestSchema>>,
    data_updated_cache: Arc<Mutex<Option<(Instant, i64)>>>,
}

/// Drop the driver's owned runtimes outside any caller's async runtime as well.
struct ManagedPool(Option<Pool>);

impl std::ops::Deref for ManagedPool {
    type Target = Pool;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("live PostgreSQL pool")
    }
}

impl Drop for ManagedPool {
    fn drop(&mut self) {
        if let Some(pool) = self.0.take() {
            crate::pg::driver_call(move || drop(pool));
        }
    }
}

#[derive(Debug)]
struct ConfigureConnection {
    schema: String,
}

impl r2d2::CustomizeConnection<postgres::Client, postgres::Error> for ConfigureConnection {
    fn on_acquire(&self, client: &mut postgres::Client) -> Result<(), postgres::Error> {
        client.query_one(
            "SELECT set_config('mpgs.schema', $1, false)",
            &[&self.schema],
        )?;
        client.query_one(
            "SELECT set_config('search_path', quote_ident($1) || ', public, extensions', false)",
            &[&self.schema],
        )?;
        client.batch_execute("SET statement_timeout = '5s'; SET lock_timeout = '2s'; SET idle_in_transaction_session_timeout = '15s'; SET TIME ZONE 'UTC'")?;
        Ok(())
    }
}

struct TestSchema {
    pool: Arc<ManagedPool>,
    name: String,
}
impl Drop for TestSchema {
    fn drop(&mut self) {
        if let Ok(mut connection) =
            crate::pg::driver_call(|| self.pool.get_timeout(Duration::from_secs(1)))
        {
            // Generated test identifiers contain only ASCII letters, digits and '_'.
            let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.name);
            let _ = crate::pg::driver_call(|| connection.batch_execute(&sql));
        }
    }
}

impl Database {
    pub fn open(database_url: impl AsRef<str>) -> StorageResult<Self> {
        Self::open_with_clock(database_url, Arc::new(SystemClock))
    }

    pub fn open_with_clock(
        database_url: impl AsRef<str>,
        clock: Arc<dyn Clock>,
    ) -> StorageResult<Self> {
        Self::connect_schema(database_url.as_ref(), "mpgs", clock, None)
    }

    fn connect_schema(
        database_url: &str,
        schema: &str,
        clock: Arc<dyn Clock>,
        test_schema: Option<Arc<TestSchema>>,
    ) -> StorageResult<Self> {
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            return Err(StorageError::validation(
                "MPGS_DATABASE_URL must be a PostgreSQL connection URL",
            ));
        }
        let mut config: Config = database_url
            .parse()
            .map_err(|error: postgres::Error| crate::pg::Error::Postgres(error))?;
        if config.get_ports().contains(&6543)
            && config.get_hosts().iter().any(
                |host| matches!(host, Host::Tcp(host) if host.ends_with(".pooler.supabase.com")),
            )
        {
            return Err(StorageError::validation(
                "use the Supabase session pooler on port 5432; transaction pooling does not preserve this application's connection state",
            ));
        }
        let local = !config.get_hosts().is_empty()
            && config.get_hosts().iter().all(|host| match host {
                Host::Tcp(host) => {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                }
                #[cfg(unix)]
                Host::Unix(_) => true,
            });
        if !local {
            if config.get_ssl_mode() == SslMode::Disable {
                return Err(StorageError::validation(
                    "TLS is required for non-loopback PostgreSQL connections",
                ));
            }
            config.ssl_mode(SslMode::Require);
        }
        config.connect_timeout(Duration::from_secs(5));
        config.application_name("lobbytally");
        let tls = postgres_tls_connector(&config)?;
        let manager = Manager::new(config, MakeTlsConnector::new(tls));
        let pool = Pool::builder()
            .max_size(4)
            .min_idle(Some(0))
            .connection_timeout(Duration::from_secs(3))
            .idle_timeout(Some(Duration::from_secs(60)))
            .max_lifetime(Some(Duration::from_secs(5 * 60)))
            .connection_customizer(Box::new(ConfigureConnection {
                schema: schema.to_owned(),
            }))
            .build(manager)
            .map_err(|error| {
                StorageError::migration(format!("cannot create PostgreSQL pool: {error}"))
            })?;
        Ok(Self {
            pool: Arc::new(ManagedPool(Some(pool))),
            database_url: database_url.into(),
            schema: schema.into(),
            clock,
            test_schema,
            data_updated_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Isolated PostgreSQL schema for behavioral tests; no in-memory SQL dialect.
    /// MPGS_TEST_DATABASE_URL must point to an expendable PostgreSQL test database.
    pub fn open_in_memory() -> StorageResult<Self> {
        Self::open_in_memory_with_clock(Arc::new(SystemClock))
    }
    pub fn open_in_memory_with_clock(clock: Arc<dyn Clock>) -> StorageResult<Self> {
        let url = std::env::var("MPGS_TEST_DATABASE_URL").map_err(|_| {
            StorageError::validation(
                "MPGS_TEST_DATABASE_URL is required for PostgreSQL integration tests",
            )
        })?;
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).map_err(|error| {
            StorageError::migration(format!("test schema randomness failed: {error}"))
        })?;
        let schema = format!(
            "mpgs_test_{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let mut database = Self::connect_schema(&url, &schema, clock, None)?;
        database.with_conn(|conn| {
            conn.execute_batch(&format!("CREATE SCHEMA \"{schema}\""))?;
            Ok(())
        })?;
        database.test_schema = Some(Arc::new(TestSchema {
            pool: database.pool.clone(),
            name: schema,
        }));
        Ok(database)
    }

    /// An independent connection pool sharing the same schema and clock.
    pub fn reconnect(&self) -> StorageResult<Self> {
        Self::connect_schema(
            &self.database_url,
            &self.schema,
            self.clock.clone(),
            self.test_schema.clone(),
        )
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }
    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn migrate(&self) -> StorageResult<i64> {
        self.with_conn_mut(|conn| migrate::migrate_to_latest(conn, self.now_ms()))
    }
    pub fn schema_version(&self) -> StorageResult<i64> {
        self.with_conn(migrate::current_version)
    }

    pub fn invalidate_data_updated_cache(&self) {
        if let Ok(mut guard) = self.data_updated_cache.lock() {
            *guard = None;
        }
    }
    pub(crate) fn cached_data_updated_at_ms(
        &self,
        compute: impl FnOnce() -> StorageResult<i64>,
    ) -> StorageResult<i64> {
        if let Ok(guard) = self.data_updated_cache.lock()
            && let Some((at, value)) = *guard
            && at.elapsed() < DATA_UPDATED_CACHE_TTL
        {
            return Ok(value);
        }
        let value = compute()?;
        if let Ok(mut guard) = self.data_updated_cache.lock() {
            *guard = Some((Instant::now(), value));
        }
        Ok(value)
    }

    pub(crate) fn cached_feed_snapshot_at_ms(
        &self,
        compute: impl FnOnce() -> StorageResult<i64>,
    ) -> StorageResult<i64> {
        let slot = self.now_ms().div_euclid(FEED_SNAPSHOT_INTERVAL_MS);
        if let Some(snapshot) = self.with_conn(|conn| {
            use crate::pg::OptionalExtension;
            Ok(conn.query_row("SELECT snapshot_ms FROM feed_snapshot_state WHERE singleton = 1 AND epoch_slot = $1", [slot], |row| row.get(0)).optional()?)
        })? { return Ok(snapshot); }
        let watermark = compute()?;
        self.with_conn(|conn| {
            // First writer for each shared time slot fixes the watermark for all instances.
            conn.execute("UPDATE feed_snapshot_state SET epoch_slot = $1, snapshot_ms = $2 WHERE singleton = 1 AND epoch_slot < $1", crate::pg::params![slot, watermark])?;
            Ok(conn.query_row("SELECT snapshot_ms FROM feed_snapshot_state WHERE singleton = 1", [], |row| row.get(0))?)
        })
    }

    #[cfg(test)]
    pub(crate) fn invalidate_feed_snapshot_cache(&self) {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE feed_snapshot_state SET epoch_slot = -1 WHERE singleton = 1",
                [],
            )?;
            Ok(())
        })
        .expect("invalidate shared feed snapshot");
    }

    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let client = crate::pg::driver_call(|| self.pool.get()).map_err(|error| {
            StorageError::migration(format!("PostgreSQL pool checkout failed: {error}"))
        })?;
        f(&Connection::new(client))
    }
    pub fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let client = crate::pg::driver_call(|| self.pool.get()).map_err(|error| {
            StorageError::migration(format!("PostgreSQL pool checkout failed: {error}"))
        })?;
        let result = f(&mut Connection::new(client));
        self.invalidate_data_updated_cache();
        result
    }

    pub fn integrity_check(&self) -> StorageResult<Vec<String>> {
        self.with_conn(|conn| {
            let invalid: i64 = conn.query_row("SELECT COUNT(*) FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace WHERE n.nspname = current_schema() AND NOT c.convalidated", [], |row| row.get(0))?;
            Ok(if invalid == 0 { vec!["ok".to_owned()] } else { vec![format!("{invalid} unvalidated constraints")] })
        })
    }
    pub fn assert_ready(&self) -> StorageResult<()> {
        self.readiness_check()
    }
    pub fn readiness_check(&self) -> StorageResult<()> {
        let version = self.schema_version()?;
        if version != latest_version() {
            return Err(StorageError::migration(format!(
                "schema version {version} != expected {}",
                latest_version()
            )));
        }
        self.with_conn(|conn| {
            conn.prepare("SELECT session_id, refresh_token_hash, revoked_at_ms FROM account_sessions LIMIT 0")?;
            conn.prepare("SELECT job_id, lease_owner, lease_expires_at_ms FROM jobs LIMIT 0")?;
            conn.prepare("SELECT app_id, canonical_name FROM apps LIMIT 0")?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supabase_ca_trust_is_scoped_to_official_database_hosts() {
        for host in [
            "aws-0-ap-northeast-1.pooler.supabase.com",
            "DB.projectref.SUPABASE.CO",
            "db.projectref.supabase.co.",
        ] {
            assert!(is_supabase_database_host(host), "{host}");
        }
        for host in [
            "127.0.0.1",
            "localhost",
            "db.projectref.supabase.co.attacker.example",
            "pooler.supabase.com.attacker.example",
            "evil-supabase.co",
            "projectref.supabase.co",
            "db..supabase.co",
        ] {
            assert!(!is_supabase_database_host(host), "{host}");
        }
    }

    #[test]
    fn bundled_supabase_ca_is_the_verified_official_certificate() {
        use sha2::{Digest, Sha256};
        let normalized = std::str::from_utf8(SUPABASE_ROOT_CA)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(
            format!("{:x}", Sha256::digest(normalized.as_bytes())),
            "700723581420dd1ac98fd7e9ac529f0ef210eadcaf87fc868a3ad7d114c2f3b7"
        );
        let config = "postgresql://test@aws-0-ap-northeast-1.pooler.supabase.com/postgres"
            .parse::<Config>()
            .unwrap();
        postgres_tls_connector(&config).unwrap();
    }

    #[test]
    #[ignore = "requires a public network connection to the Supabase session pooler"]
    fn supabase_pooler_tls_verifies_chain_and_rejects_a_wrong_hostname() {
        use std::io::{Read, Write};
        use std::net::{TcpStream, ToSocketAddrs};
        const HOST: &str = "aws-0-ap-northeast-1.pooler.supabase.com";

        fn postgres_tls_socket() -> TcpStream {
            let mut socket = (HOST, 5432)
                .to_socket_addrs()
                .unwrap()
                .find_map(|address| {
                    TcpStream::connect_timeout(&address, Duration::from_secs(5)).ok()
                })
                .expect("connect to the public Supabase pooler");
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            // PostgreSQL SSLRequest; no StartupMessage, username or password is sent.
            socket.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).unwrap();
            let mut response = [0];
            socket.read_exact(&mut response).unwrap();
            assert_eq!(response, [b'S']);
            socket
        }

        let config = format!("postgresql://test@{HOST}/postgres")
            .parse::<Config>()
            .unwrap();
        let connector = postgres_tls_connector(&config).unwrap();
        let stream = connector.connect(HOST, postgres_tls_socket()).unwrap();
        assert!(stream.peer_certificate().unwrap().is_some());
        drop(stream);
        assert!(
            connector
                .connect("hostname-mismatch.invalid", postgres_tls_socket())
                .is_err()
        );
    }

    #[test]
    fn invalid_connection_locations_are_rejected_without_connecting() {
        assert!(Database::open("local.sqlite").is_err());
        assert!(
            Database::open("postgresql://user:password@example.com/postgres?sslmode=disable")
                .is_err()
        );
        assert!(
            Database::open(
                "postgresql://user:password@aws-0-ap-northeast-1.pooler.supabase.com:6543/postgres"
            )
            .is_err()
        );
    }
    #[test]
    fn shared_schema_survives_a_single_handle_and_has_isolated_test_lifetime() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let other = db.reconnect().unwrap();
        drop(db);
        other.assert_ready().unwrap();
    }
}
