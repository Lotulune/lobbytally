//! Private Supabase Storage objects. Public avatar access remains an API decision.

use std::{env, io, sync::Arc, time::Duration};

use reqwest::{Client, StatusCode, Url, header};

const MAX_PROCESSED_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct AvatarStorage {
    backend: Arc<Backend>,
}

enum Backend {
    Supabase {
        client: Client,
        base_url: Url,
        bucket: String,
    },
    #[cfg(test)]
    Memory(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>),
}

impl AvatarStorage {
    pub fn from_env() -> Result<Self, io::Error> {
        let url = required_env("MPGS_SUPABASE_URL")?;
        let key = required_env("MPGS_SUPABASE_SERVICE_ROLE_KEY")?;
        let bucket = env::var("MPGS_SUPABASE_AVATAR_BUCKET").unwrap_or_else(|_| "avatars".into());
        Self::supabase(&url, &key, &bucket)
    }

    fn supabase(url: &str, key: &str, bucket: &str) -> Result<Self, io::Error> {
        let mut base_url = Url::parse(url).map_err(|_| invalid("MPGS_SUPABASE_URL is invalid"))?;
        if base_url.scheme() != "https"
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(invalid("MPGS_SUPABASE_URL must be an HTTPS project origin"));
        }
        if bucket.is_empty()
            || bucket.len() > 63
            || !bucket
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(invalid("MPGS_SUPABASE_AVATAR_BUCKET must be a bucket name"));
        }
        base_url.set_path("/storage/v1/");
        let mut headers = header::HeaderMap::new();
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| invalid("MPGS_SUPABASE_SERVICE_ROLE_KEY contains invalid bytes"))?;
        authorization.set_sensitive(true);
        let mut api_key = header::HeaderValue::from_str(key)
            .map_err(|_| invalid("MPGS_SUPABASE_SERVICE_ROLE_KEY contains invalid bytes"))?;
        api_key.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, authorization);
        headers.insert("apikey", api_key);
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid("could not configure Supabase Storage client"))?;
        Ok(Self {
            backend: Arc::new(Backend::Supabase {
                client,
                base_url,
                bucket: bucket.to_owned(),
            }),
        })
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            backend: Arc::new(Backend::Memory(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            ))),
        }
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), String> {
        validate_key(key)?;
        if bytes.is_empty() || bytes.len() > MAX_PROCESSED_BYTES {
            return Err("processed avatar exceeds the storage size limit".into());
        }
        match self.backend.as_ref() {
            Backend::Supabase {
                client,
                base_url,
                bucket,
            } => {
                let url = object_url(base_url, bucket, key, false)?;
                let response = client
                    .post(url)
                    .header("x-upsert", "true")
                    .header(header::CONTENT_TYPE, "image/webp")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(bytes)
                    .send()
                    .await
                    .map_err(|_| "avatar upload request failed".to_owned())?;
                expect_success(response.status(), "upload")
            }
            #[cfg(test)]
            Backend::Memory(objects) => {
                objects.lock().unwrap().insert(key.to_owned(), bytes);
                Ok(())
            }
        }
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        validate_key(key)?;
        match self.backend.as_ref() {
            Backend::Supabase {
                client,
                base_url,
                bucket,
            } => {
                let url = object_url(base_url, bucket, key, true)?;
                let mut response = client
                    .get(url)
                    .send()
                    .await
                    .map_err(|_| "avatar download request failed".to_owned())?;
                if response.status() == StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                expect_success(response.status(), "download")?;
                if response
                    .content_length()
                    .is_some_and(|length| length > MAX_PROCESSED_BYTES as u64)
                {
                    return Err("stored avatar exceeds the size limit".into());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "avatar download was interrupted".to_owned())?
                {
                    if bytes.len().saturating_add(chunk.len()) > MAX_PROCESSED_BYTES {
                        return Err("stored avatar exceeds the size limit".into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(Some(bytes))
            }
            #[cfg(test)]
            Backend::Memory(objects) => Ok(objects.lock().unwrap().get(key).cloned()),
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), String> {
        validate_key(key)?;
        match self.backend.as_ref() {
            Backend::Supabase {
                client,
                base_url,
                bucket,
            } => {
                let url = base_url
                    .join(&format!("object/{bucket}"))
                    .map_err(|_| "invalid avatar storage URL".to_owned())?;
                let response = client
                    .delete(url)
                    .json(&serde_json::json!({"prefixes": [key]}))
                    .send()
                    .await
                    .map_err(|_| "avatar delete request failed".to_owned())?;
                // The batch remove endpoint treats already-missing keys as success.
                expect_success(response.status(), "delete")
            }
            #[cfg(test)]
            Backend::Memory(objects) => {
                objects.lock().unwrap().remove(key);
                Ok(())
            }
        }
    }
}

fn object_url(base: &Url, bucket: &str, key: &str, authenticated: bool) -> Result<Url, String> {
    let prefix = if authenticated {
        "object/authenticated"
    } else {
        "object"
    };
    base.join(&format!("{prefix}/{bucket}/{key}"))
        .map_err(|_| "invalid avatar storage URL".to_owned())
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 160
        || !key.ends_with(".webp")
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("invalid avatar storage key".into());
    }
    Ok(())
}

fn expect_success(status: StatusCode, operation: &str) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else {
        Err(format!(
            "avatar {operation} failed with HTTP {}",
            status.as_u16()
        ))
    }
}

fn required_env(name: &str) -> Result<String, io::Error> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(&format!("{name} is required")))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_configuration_rejects_redirectable_origins_and_invalid_bucket() {
        assert!(AvatarStorage::supabase("http://example.com", "key", "avatars").is_err());
        assert!(AvatarStorage::supabase("https://example.com/path", "key", "avatars").is_err());
        assert!(AvatarStorage::supabase("https://example.com", "key", "../avatars").is_err());
        assert!(AvatarStorage::supabase("https://example.com", "key", "avatars").is_ok());
    }

    #[tokio::test]
    async fn storage_roundtrip_enforces_processed_size_and_safe_keys() {
        let storage = AvatarStorage::memory();
        assert!(storage.put("../a.webp", vec![1]).await.is_err());
        assert!(
            storage
                .put("a.webp", vec![0; MAX_PROCESSED_BYTES + 1])
                .await
                .is_err()
        );
        storage.put("a.webp", vec![1, 2]).await.unwrap();
        assert_eq!(storage.get("a.webp").await.unwrap(), Some(vec![1, 2]));
        storage.delete("a.webp").await.unwrap();
        assert_eq!(storage.get("a.webp").await.unwrap(), None);
    }

    #[tokio::test]
    async fn supabase_requests_use_private_authenticated_routes_and_delete_prefixes() {
        use axum::{
            Router,
            body::Bytes,
            http::HeaderMap,
            routing::{delete, get, post},
        };
        let app = Router::new()
            .route(
                "/storage/v1/object/avatars/user_hash.webp",
                post(|headers: HeaderMap, body: Bytes| async move {
                    assert_eq!(
                        headers.get(header::AUTHORIZATION).unwrap(),
                        "Bearer test-service-key"
                    );
                    assert_eq!(headers.get("apikey").unwrap(), "test-service-key");
                    assert_eq!(headers.get("x-upsert").unwrap(), "true");
                    assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "image/webp");
                    assert_eq!(body.as_ref(), &[1, 2, 3]);
                    StatusCode::OK
                }),
            )
            .route(
                "/storage/v1/object/authenticated/avatars/user_hash.webp",
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers.get(header::AUTHORIZATION).unwrap(),
                        "Bearer test-service-key"
                    );
                    vec![1_u8, 2, 3]
                }),
            )
            .route(
                "/storage/v1/object/avatars",
                delete(
                    |axum::Json(body): axum::Json<serde_json::Value>| async move {
                        assert_eq!(body, serde_json::json!({"prefixes": ["user_hash.webp"]}));
                        StatusCode::OK
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer test-service-key".parse().unwrap(),
        );
        headers.insert("apikey", "test-service-key".parse().unwrap());
        let storage = AvatarStorage {
            backend: Arc::new(Backend::Supabase {
                client: Client::builder()
                    .default_headers(headers)
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap(),
                base_url: Url::parse(&format!("http://{address}/storage/v1/")).unwrap(),
                bucket: "avatars".into(),
            }),
        };
        storage.put("user_hash.webp", vec![1, 2, 3]).await.unwrap();
        assert_eq!(
            storage.get("user_hash.webp").await.unwrap(),
            Some(vec![1, 2, 3])
        );
        storage.delete("user_hash.webp").await.unwrap();
        server.abort();
    }
}
