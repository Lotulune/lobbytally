//! Small synchronous PostgreSQL query layer used by the repository.
//!
//! Statements are PostgreSQL SQL, including numbered `$n` parameters. This
//! module never rewrites SQL. It centralizes checked scalar conversions and
//! transaction/savepoint ownership so repository operations stay synchronous.

use std::{
    cell::{Cell, RefCell},
    ops::Deref,
};

use bytes::BytesMut;
use postgres::types::{IsNull, ToSql as PgToSql, Type as PgType};
use postgres_native_tls::MakeTlsConnector;
use r2d2_postgres::PostgresConnectionManager;
use rust_decimal::{Decimal, prelude::ToPrimitive};

pub(crate) type Manager = PostgresConnectionManager<MakeTlsConnector>;
pub(crate) type Pool = r2d2::Pool<Manager>;

/// The synchronous driver owns a Tokio runtime. Production callers use
/// spawn_blocking; this also supports direct calls from an async integration
/// test or a bounded permit destructor without nesting runtimes.
pub(crate) fn driver_call<T: Send>(call: impl FnOnce() -> T + Send) -> T {
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|scope| {
            scope
                .spawn(call)
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    } else {
        call()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Postgres(#[from] postgres::Error),
    #[error("query returned no rows")]
    QueryReturnedNoRows,
    #[error("column {0} ({1:?}) could not be decoded: {2}")]
    FromSqlConversionFailure(usize, types::Type, Box<dyn std::error::Error + Send + Sync>),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod types {
    #[derive(Debug, Clone, Copy)]
    pub enum Type {
        Null,
        Integer,
        Real,
        Text,
        Blob,
    }

    #[derive(Debug, Clone)]
    pub enum Value {
        Null,
        Integer(i64),
        Real(f64),
        Text(String),
        Blob(Vec<u8>),
        Bool(bool),
    }
}

use types::Value;

pub trait ToSql {
    fn to_value(&self) -> Value;
}

impl ToSql for Value {
    fn to_value(&self) -> Value {
        self.clone()
    }
}
impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_value(&self) -> Value {
        (*self).to_value()
    }
}
impl ToSql for str {
    fn to_value(&self) -> Value {
        Value::Text(self.to_owned())
    }
}
impl ToSql for String {
    fn to_value(&self) -> Value {
        Value::Text(self.clone())
    }
}
impl ToSql for Vec<u8> {
    fn to_value(&self) -> Value {
        Value::Blob(self.clone())
    }
}
impl ToSql for [u8] {
    fn to_value(&self) -> Value {
        Value::Blob(self.to_vec())
    }
}
impl ToSql for bool {
    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }
}
impl ToSql for f64 {
    fn to_value(&self) -> Value {
        Value::Real(*self)
    }
}
impl ToSql for f32 {
    fn to_value(&self) -> Value {
        Value::Real(f64::from(*self))
    }
}
impl<T: ToSql> ToSql for Option<T> {
    fn to_value(&self) -> Value {
        self.as_ref().map(ToSql::to_value).unwrap_or(Value::Null)
    }
}

macro_rules! integer_values {
    ($($ty:ty),*) => { $(
        impl ToSql for $ty { fn to_value(&self) -> Value { Value::Integer(i64::from(*self)) } }
        impl From<$ty> for Value { fn from(value: $ty) -> Self { value.to_value() } }
    )* };
}
integer_values!(i8, i16, i32, i64, u8, u16, u32);
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}
impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

impl PgToSql for Value {
    fn to_sql(
        &self,
        ty: &PgType,
        output: &mut BytesMut,
    ) -> std::result::Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::Null => Ok(IsNull::Yes),
            Self::Integer(value) => match *ty {
                PgType::INT2 => i16::try_from(*value)?.to_sql(ty, output),
                PgType::INT4 => i32::try_from(*value)?.to_sql(ty, output),
                PgType::INT8 => value.to_sql(ty, output),
                PgType::FLOAT4 => (*value as f32).to_sql(ty, output),
                PgType::FLOAT8 => (*value as f64).to_sql(ty, output),
                PgType::NUMERIC => Decimal::from(*value).to_sql(ty, output),
                PgType::BOOL => (*value != 0).to_sql(ty, output),
                _ => Err(format!("integer parameter cannot bind as {ty}").into()),
            },
            Self::Real(value) => match *ty {
                PgType::FLOAT4 => (*value as f32).to_sql(ty, output),
                PgType::FLOAT8 => value.to_sql(ty, output),
                PgType::NUMERIC => Decimal::from_f64_retain(*value)
                    .ok_or("non-finite numeric parameter")?
                    .to_sql(ty, output),
                _ => Err(format!("real parameter cannot bind as {ty}").into()),
            },
            Self::Bool(value) if *ty == PgType::BOOL => value.to_sql(ty, output),
            Self::Bool(value) => Self::Integer(i64::from(*value)).to_sql(ty, output),
            Self::Text(value) => value.to_sql(ty, output),
            Self::Blob(value) => value.to_sql(ty, output),
        }
    }
    fn accepts(_: &PgType) -> bool {
        true
    }
    postgres::types::to_sql_checked!();
}

pub trait Params {
    fn values(self) -> Vec<Value>;
}
impl Params for [(); 0] {
    fn values(self) -> Vec<Value> {
        Vec::new()
    }
}
impl Params for Vec<Value> {
    fn values(self) -> Vec<Value> {
        self
    }
}
impl Params for &[Value] {
    fn values(self) -> Vec<Value> {
        self.to_vec()
    }
}
macro_rules! array_params { ($($n:expr),*) => { $(
    impl<T: ToSql> Params for [T; $n] { fn values(self) -> Vec<Value> { self.iter().map(ToSql::to_value).collect() } }
)* }; }
array_params!(
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32
);

#[macro_export]
macro_rules! pg_params {
    ($($value:expr),* $(,)?) => { vec![$($crate::pg::ToSql::to_value(&$value)),*] };
}
pub use crate::pg_params as params;
pub fn params_from_iter<T: ToSql>(values: impl IntoIterator<Item = T>) -> Vec<Value> {
    values.into_iter().map(|value| value.to_value()).collect()
}

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub trait RowIndex {
    fn index(&self, row: &postgres::Row) -> Option<usize>;
}
impl RowIndex for usize {
    fn index(&self, row: &postgres::Row) -> Option<usize> {
        (*self < row.len()).then_some(*self)
    }
}
impl RowIndex for &str {
    fn index(&self, row: &postgres::Row) -> Option<usize> {
        row.columns()
            .iter()
            .position(|column| column.name() == *self)
    }
}

pub struct Row<'a>(&'a postgres::Row);
impl Row<'_> {
    pub fn get<I: RowIndex, T: FromSql>(&self, index: I) -> Result<T> {
        let index = index
            .index(self.0)
            .ok_or_else(|| Error::Invalid("column does not exist".to_owned()))?;
        T::from_row(self.0, index)
    }
}

pub trait FromSql: Sized {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self>;
}
fn invalid_column(index: usize, expected: &str) -> Error {
    Error::Invalid(format!("column {index} cannot be decoded as {expected}"))
}

impl FromSql for i64 {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        match *row.columns()[index].type_() {
            PgType::INT2 => Ok(i64::from(row.try_get::<_, i16>(index)?)),
            PgType::INT4 => Ok(i64::from(row.try_get::<_, i32>(index)?)),
            PgType::INT8 => Ok(row.try_get(index)?),
            PgType::BOOL => Ok(i64::from(row.try_get::<_, bool>(index)?)),
            PgType::NUMERIC => {
                let value: Decimal = row.try_get(index)?;
                if !value.fract().is_zero() {
                    return Err(invalid_column(index, "integer"));
                }
                value.to_i64().ok_or_else(|| invalid_column(index, "i64"))
            }
            _ => Err(invalid_column(index, "integer")),
        }
    }
}
macro_rules! checked_integers { ($($ty:ty),*) => { $(
    impl FromSql for $ty { fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        Self::try_from(i64::from_row(row, index)?).map_err(|error| Error::FromSqlConversionFailure(index, types::Type::Integer, Box::new(error)))
    } }
)* }; }
checked_integers!(i8, i16, i32, u8, u16, u32, u64, usize);
impl FromSql for f64 {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        match *row.columns()[index].type_() {
            PgType::FLOAT4 => Ok(f64::from(row.try_get::<_, f32>(index)?)),
            PgType::FLOAT8 => Ok(row.try_get(index)?),
            PgType::NUMERIC => row
                .try_get::<_, Decimal>(index)?
                .to_f64()
                .ok_or_else(|| invalid_column(index, "f64")),
            _ => Ok(i64::from_row(row, index)? as f64),
        }
    }
}
impl FromSql for bool {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        Ok(i64::from_row(row, index)? != 0)
    }
}
impl FromSql for String {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        Ok(row.try_get(index)?)
    }
}
impl FromSql for Vec<u8> {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        Ok(row.try_get(index)?)
    }
}
#[derive(Debug)]
struct NullProbe;
impl<'a> postgres::types::FromSql<'a> for NullProbe {
    fn from_sql(
        _: &PgType,
        _: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self)
    }
    fn accepts(_: &PgType) -> bool {
        true
    }
}
impl<T: FromSql> FromSql for Option<T> {
    fn from_row(row: &postgres::Row, index: usize) -> Result<Self> {
        if row.try_get::<_, Option<NullProbe>>(index)?.is_none() {
            Ok(None)
        } else {
            T::from_row(row, index).map(Some)
        }
    }
}

pub struct Connection {
    client: RefCell<r2d2::PooledConnection<Manager>>,
    transaction_depth: Cell<usize>,
}
impl Connection {
    pub(crate) fn new(client: r2d2::PooledConnection<Manager>) -> Self {
        Self {
            client: RefCell::new(client),
            transaction_depth: Cell::new(0),
        }
    }
    fn with_client<T: Send>(&self, call: impl FnOnce(&mut postgres::Client) -> T + Send) -> T {
        let mut guard = self.client.borrow_mut();
        let client: &mut postgres::Client = &mut guard;
        driver_call(|| call(client))
    }
    pub fn execute(&self, sql: &str, params: impl Params) -> Result<usize> {
        let values = params.values();
        let refs: Vec<&(dyn PgToSql + Sync)> = values.iter().map(|value| value as _).collect();
        Ok(self.with_client(|client| client.execute(sql, &refs))? as usize)
    }
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.with_client(|client| client.batch_execute(sql))?;
        Ok(())
    }
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        let statement = self.with_client(|client| client.prepare(sql))?;
        Ok(Statement {
            conn: self,
            statement,
        })
    }
    pub fn query_row<T>(
        &self,
        sql: &str,
        params: impl Params,
        map: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        let rows = self.prepare(sql)?.rows(params)?;
        let row = rows.first().ok_or(Error::QueryReturnedNoRows)?;
        map(&Row(row))
    }
    pub fn transaction(&self) -> Result<Transaction<'_>> {
        Transaction::new_unchecked(self, TransactionBehavior::Immediate)
    }
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.transaction()
    }
    pub fn transaction_with_behavior(
        &self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        Transaction::new_unchecked(self, behavior)
    }
    pub fn read_transaction(&self) -> Result<Transaction<'_>> {
        Transaction::begin(self, false, true)
    }
}

pub struct Statement<'a> {
    conn: &'a Connection,
    statement: postgres::Statement,
}
impl Statement<'_> {
    fn rows(&self, params: impl Params) -> Result<Vec<postgres::Row>> {
        let values = params.values();
        let refs: Vec<&(dyn PgToSql + Sync)> = values.iter().map(|value| value as _).collect();
        let statement = &self.statement;
        Ok(self
            .conn
            .with_client(|client| client.query(statement, &refs))?)
    }
    pub fn query_map<T>(
        &mut self,
        params: impl Params,
        mut map: impl FnMut(&Row<'_>) -> Result<T>,
    ) -> Result<std::vec::IntoIter<Result<T>>> {
        Ok(self
            .rows(params)?
            .iter()
            .map(|row| map(&Row(row)))
            .collect::<Vec<_>>()
            .into_iter())
    }
    pub fn query_row<T>(
        &mut self,
        params: impl Params,
        map: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        let rows = self.rows(params)?;
        map(&Row(rows.first().ok_or(Error::QueryReturnedNoRows)?))
    }
}
#[derive(Clone, Copy)]
pub enum TransactionBehavior {
    Deferred,
    Immediate,
}
pub struct Transaction<'a> {
    conn: &'a Connection,
    depth: usize,
    completed: bool,
}
impl<'a> Transaction<'a> {
    pub fn new_unchecked(conn: &'a Connection, behavior: TransactionBehavior) -> Result<Self> {
        Self::begin(
            conn,
            matches!(behavior, TransactionBehavior::Immediate),
            false,
        )
    }
    fn begin(conn: &'a Connection, serialized: bool, read_only: bool) -> Result<Self> {
        let depth = conn.transaction_depth.get();
        if depth == 0 {
            conn.execute_batch(if read_only {
                "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"
            } else {
                "BEGIN"
            })?;
        } else {
            conn.execute_batch(&format!("SAVEPOINT mpgs_transaction_{depth}"))?;
        }
        conn.transaction_depth.set(depth + 1);
        let tx = Self {
            conn,
            depth,
            completed: false,
        };
        if serialized {
            conn.query_row("SELECT pg_advisory_xact_lock(hashtextextended(current_setting('mpgs.schema') || ':writes', 0))", [], |_| Ok(()))?;
        }
        Ok(tx)
    }
    pub fn commit(mut self) -> Result<()> {
        self.conn.execute_batch(&if self.depth == 0 {
            "COMMIT".to_owned()
        } else {
            format!("RELEASE SAVEPOINT mpgs_transaction_{}", self.depth)
        })?;
        self.conn.transaction_depth.set(self.depth);
        self.completed = true;
        Ok(())
    }
    pub fn rollback(mut self) -> Result<()> {
        self.rollback_inner()?;
        self.completed = true;
        Ok(())
    }
    fn rollback_inner(&self) -> Result<()> {
        let sql = if self.depth == 0 {
            "ROLLBACK".to_owned()
        } else {
            format!(
                "ROLLBACK TO SAVEPOINT mpgs_transaction_{}; RELEASE SAVEPOINT mpgs_transaction_{}",
                self.depth, self.depth
            )
        };
        let result = self.conn.execute_batch(&sql);
        self.conn.transaction_depth.set(self.depth);
        result
    }
}
impl Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        self.conn
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.rollback_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[test]
    fn native_bindings_preserve_text_nulls_numeric_widths_and_binary_values() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let text = "中文'; SELECT 42; --";
            let row: (String, i64, f64, bool, Option<i64>, Vec<u8>) = conn.query_row(
                "SELECT $1::text, $2::integer, SUM(value), EXISTS(SELECT 1), NULL, $3::bytea FROM (VALUES (12::bigint), (34::bigint)) sample(value)",
                params![text, 123_i64, vec![0_u8, 255, 42]],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )?;
            assert_eq!(row, (text.to_owned(), 123, 46.0, true, None, vec![0, 255, 42]));
            assert!(conn.query_row("SELECT 2147483648::bigint", [], |row| row.get::<_, i32>(0)).is_err());
            Ok(())
        }).unwrap();
    }

    #[test]
    fn nested_savepoint_rollback_preserves_parent_transaction() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TABLE transaction_probe (value bigint PRIMARY KEY)")?;
            let outer = conn.transaction()?;
            outer.execute("INSERT INTO transaction_probe VALUES ($1)", [1_i64])?;
            {
                let nested = outer.transaction()?;
                nested.execute("INSERT INTO transaction_probe VALUES ($1)", [2_i64])?;
            }
            outer.commit()?;
            let values = conn
                .prepare("SELECT value FROM transaction_probe ORDER BY value")?
                .query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>>>()?;
            assert_eq!(values, vec![1]);
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn direct_async_test_calls_do_not_nest_driver_runtimes() {
        let db = Database::open_in_memory().unwrap();
        let value = db
            .with_conn(|conn| Ok(conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?))
            .unwrap();
        assert_eq!(value, 1);
    }
}
