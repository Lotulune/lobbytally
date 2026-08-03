//! M7 account identity, session, profile, avatar metadata, and encrypted AI
//! credential storage. Anonymous subjects remain the stable foreign-key root;
//! an account is an extension of one subject rather than a replacement for it.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use mpgs_domain::UserPreferences;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::{StorageError, StorageResult};
use crate::users::{ACCESS_TOKEN_TTL_MS, REFRESH_TOKEN_TTL_MS, token_hash};

const PASSWORD_SCHEME: &str = "argon2id-v19";
// A syntactically valid Argon2id PHC string with an intentionally impossible
// all-zero digest. Unknown users verify against this value so their login path
// performs the same expensive password work as a known active account.
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c3RhdGljLWR1bW15LXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const SESSION_PREFIX: &str = "as_";
const ACCESS_PREFIX: &str = "mpgs_acct_at_";
const REFRESH_PREFIX: &str = "mpgs_acct_rt_";
const AVATAR_PREFIX: &str = "av_";
const CIPHER_VERSION: u8 = 1;
const CIPHER_NONCE_BYTES: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSessionTokens {
    pub(crate) session_id: String,
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub refresh_expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterAccount {
    pub username: String,
    pub display_name: String,
    pub password: String,
    pub device_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAccount {
    pub username: String,
    pub password: String,
    pub device_label: String,
    pub preference_choice: Option<PreferenceChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceChoice {
    Anonymous,
    Account,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProfile {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub status: String,
    pub avatar_public_id: String,
    pub avatar_version: u32,
    pub avatar_storage_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAvatar {
    pub display_name: String,
    pub avatar_public_id: String,
    pub avatar_version: u32,
    pub storage_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarMetadata {
    pub user_id: String,
    pub version: u32,
    pub content_hash: String,
    pub media_type: String,
    pub storage_key: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarLookup {
    pub display_name: String,
    pub version: u32,
    pub storage_key: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiMode {
    Builtin,
    Custom,
    Off,
}

impl AiMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "builtin" => Some(Self::Builtin),
            "custom" => Some(Self::Custom),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSettings {
    pub mode: AiMode,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub configured: bool,
    pub key_mask: Option<String>,
    pub updated_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutAiSettings {
    pub mode: AiMode,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Omitted preserves an existing custom key. It is never returned by an API.
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAiCredential {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

/// AES-256-GCM wrapper used only with a deployment-owned master key. The key
/// is intentionally neither serializable nor printable.
#[derive(Clone)]
pub struct CredentialCipher {
    key: [u8; 32],
}

impl std::fmt::Debug for CredentialCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CredentialCipher([redacted])")
    }
}

impl CredentialCipher {
    pub fn from_hex(raw: &str) -> StorageResult<Self> {
        let raw = raw.trim();
        if raw.len() != 64 {
            return Err(StorageError::validation(
                "AI master key must be exactly 32 bytes encoded as 64 hexadecimal characters",
            ));
        }
        let mut key = [0_u8; 32];
        for (index, byte) in key.iter_mut().enumerate() {
            let offset = index * 2;
            *byte = hex_pair(&raw[offset..offset + 2]).ok_or_else(|| {
                StorageError::validation("AI master key must contain only hexadecimal characters")
            })?;
        }
        Ok(Self { key })
    }

    #[cfg(test)]
    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn encrypt(&self, plaintext: &str) -> StorageResult<Vec<u8>> {
        let mut nonce = [0_u8; CIPHER_NONCE_BYTES];
        getrandom::fill(&mut nonce)
            .map_err(|error| StorageError::Io(std::io::Error::other(error.to_string())))?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| StorageError::validation("invalid AI master key"))?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| StorageError::validation("unable to encrypt AI API key"))?;
        let mut encoded = Vec::with_capacity(1 + nonce.len() + ciphertext.len());
        encoded.push(CIPHER_VERSION);
        encoded.extend_from_slice(&nonce);
        encoded.extend_from_slice(&ciphertext);
        Ok(encoded)
    }

    pub fn decrypt(&self, encoded: &[u8]) -> StorageResult<String> {
        if encoded.len() <= 1 + CIPHER_NONCE_BYTES || encoded.first() != Some(&CIPHER_VERSION) {
            return Err(StorageError::validation(
                "stored AI credential has an unsupported format",
            ));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| StorageError::validation("invalid AI master key"))?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&encoded[1..1 + CIPHER_NONCE_BYTES]),
                &encoded[1 + CIPHER_NONCE_BYTES..],
            )
            .map_err(|_| StorageError::validation("unable to decrypt AI API key"))?;
        String::from_utf8(plaintext)
            .map_err(|_| StorageError::validation("stored AI credential is not valid UTF-8"))
    }
}

pub fn register_account(
    conn: &mut Connection,
    input: &RegisterAccount,
    anonymous_user_id: Option<&str>,
    now_ms: i64,
) -> StorageResult<AccountSessionTokens> {
    let username = normalize_username(&input.username)?;
    validate_display_name(&input.display_name)?;
    validate_password(&input.password)?;
    let device_label = normalize_device_label(&input.device_label);
    let password_hash = hash_password(&input.password)?;

    let tx = conn.transaction()?;
    let username_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE username_normalized = ?1)",
        params![username],
        |row| row.get(0),
    )?;
    if username_exists {
        return Err(StorageError::conflict("username is already registered"));
    }

    let user_id = match anonymous_user_id {
        Some(user_id) => {
            ensure_anonymous_subject(&tx, user_id)?;
            let already_account: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1)",
                params![user_id],
                |row| row.get(0),
            )?;
            if already_account {
                return Err(StorageError::conflict(
                    "current session already belongs to an account",
                ));
            }
            user_id.to_owned()
        }
        None => create_anonymous_subject(&tx, now_ms)?,
    };
    let avatar_public_id = format!("{AVATAR_PREFIX}{}", random_hex::<16>()?);
    tx.execute(
        "INSERT INTO user_accounts (
            user_id, username_normalized, display_name, password_hash, password_scheme,
            status, avatar_public_id, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?7, ?7)",
        params![
            user_id,
            username,
            input.display_name.trim(),
            password_hash,
            PASSWORD_SCHEME,
            avatar_public_id,
            now_ms
        ],
    )?;
    let session = create_account_session(&tx, &user_id, &device_label, now_ms)?;
    tx.commit()?;
    Ok(session)
}

pub fn login_account(
    conn: &mut Connection,
    input: &LoginAccount,
    anonymous_user_id: Option<&str>,
    now_ms: i64,
) -> StorageResult<AccountSessionTokens> {
    let username = normalize_username(&input.username)?;
    let device_label = normalize_device_label(&input.device_label);
    let tx = conn.transaction()?;
    let account = tx
        .query_row(
            "SELECT user_id, password_hash, password_scheme, status
             FROM user_accounts WHERE username_normalized = ?1",
            params![username],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let (verification_hash, verification_scheme) = account
        .as_ref()
        .filter(|account| account.2 == PASSWORD_SCHEME)
        .map(|account| (account.1.as_str(), account.2.as_str()))
        .unwrap_or((DUMMY_PASSWORD_HASH, PASSWORD_SCHEME));
    let password_matches =
        verify_password(verification_hash, verification_scheme, &input.password)?;
    // The caller deliberately maps this generic error to the same 401 for
    // unknown usernames, wrong passwords, frozen, and deleted accounts.
    let Some(account) = account else {
        return Err(StorageError::not_found("account credentials"));
    };
    if account.3 != "active" || !password_matches {
        return Err(StorageError::not_found("account credentials"));
    }

    if let Some(anonymous_user_id) = anonymous_user_id.filter(|source| *source != account.0) {
        ensure_anonymous_subject(&tx, anonymous_user_id)?;
        let source_is_account: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1)",
            params![anonymous_user_id],
            |row| row.get(0),
        )?;
        if !source_is_account {
            merge_anonymous_subject(&tx, anonymous_user_id, &account.0, input.preference_choice)?;
        }
    }

    let session = create_account_session(&tx, &account.0, &device_label, now_ms)?;
    tx.commit()?;
    Ok(session)
}

pub fn refresh_account_session(
    conn: &mut Connection,
    refresh_token: &str,
    now_ms: i64,
) -> StorageResult<AccountSessionTokens> {
    if refresh_token.trim().is_empty() {
        return Err(StorageError::not_found("account session"));
    }
    let refresh_hash = token_hash(refresh_token);
    let tx = conn.transaction()?;
    let session = tx
        .query_row(
            "SELECT s.session_id, s.user_id, s.revoked_at_ms, s.refresh_expires_at_ms, a.status
             FROM account_sessions AS s
             JOIN user_accounts AS a ON a.user_id = s.user_id
             WHERE s.refresh_token_hash = ?1",
            params![refresh_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((session_id, user_id, revoked_at_ms, refresh_expires_at_ms, status)) = session else {
        return Err(StorageError::not_found("account session"));
    };
    if revoked_at_ms.is_some() {
        // A reused rotated token indicates credential theft. Revoke every
        // refresh session for the account before returning a generic 401.
        tx.execute(
            "UPDATE account_sessions SET revoked_at_ms = COALESCE(revoked_at_ms, ?1)
             WHERE user_id = ?2",
            params![now_ms, user_id],
        )?;
        tx.commit()?;
        return Err(StorageError::not_found("account session"));
    }
    if refresh_expires_at_ms <= now_ms || status != "active" {
        tx.execute(
            "UPDATE account_sessions SET revoked_at_ms = COALESCE(revoked_at_ms, ?1)
             WHERE session_id = ?2",
            params![now_ms, session_id],
        )?;
        tx.commit()?;
        return Err(StorageError::not_found("account session"));
    }

    let replacement = create_account_session(&tx, &user_id, "Refreshed session", now_ms)?;
    let changed = tx.execute(
        "UPDATE account_sessions
         SET revoked_at_ms = ?1, replaced_by_session_id = ?2
         WHERE session_id = ?3 AND revoked_at_ms IS NULL",
        params![now_ms, replacement.session_id, session_id],
    )?;
    if changed != 1 {
        return Err(StorageError::not_found("account session"));
    }
    tx.commit()?;
    Ok(replacement)
}

pub fn resolve_account_user_id(
    conn: &Connection,
    access_token: &str,
    now_ms: i64,
) -> StorageResult<String> {
    let access_hash = token_hash(access_token);
    let user_id = conn
        .query_row(
            "SELECT s.user_id
             FROM account_sessions AS s
             JOIN user_accounts AS a ON a.user_id = s.user_id
             WHERE s.access_token_hash = ?1 AND s.expires_at_ms > ?2
               AND s.revoked_at_ms IS NULL AND a.status = 'active'",
            params![access_hash, now_ms],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::not_found("account session"))?;
    conn.execute(
        "UPDATE anonymous_users SET last_active_at_ms = ?1
         WHERE user_id = ?2 AND last_active_at_ms <= ?3",
        params![now_ms, user_id, now_ms.saturating_sub(5 * 60 * 1000)],
    )?;
    Ok(user_id)
}

pub fn resolve_anonymous_user_id(
    conn: &Connection,
    access_token: &str,
    now_ms: i64,
) -> StorageResult<String> {
    crate::users::resolve_user_id(conn, access_token, now_ms)
}

pub fn account_profile(conn: &Connection, user_id: &str) -> StorageResult<AccountProfile> {
    conn.query_row(
        "SELECT a.user_id, a.username_normalized, a.display_name, a.status, a.avatar_public_id,
                COALESCE(v.version, 0), v.storage_key
         FROM user_accounts AS a
         LEFT JOIN user_avatars AS v ON v.user_id = a.user_id
         WHERE a.user_id = ?1",
        params![user_id],
        |row| {
            Ok(AccountProfile {
                user_id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                status: row.get(3)?,
                avatar_public_id: row.get(4)?,
                avatar_version: row.get::<_, i64>(5)?.max(0) as u32,
                avatar_storage_key: row.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| StorageError::not_found("account"))
}

pub fn update_display_name(
    conn: &Connection,
    user_id: &str,
    display_name: &str,
    now_ms: i64,
) -> StorageResult<AccountProfile> {
    validate_display_name(display_name)?;
    let changed = conn.execute(
        "UPDATE user_accounts SET display_name = ?1, updated_at_ms = ?2
         WHERE user_id = ?3 AND status = 'active'",
        params![display_name.trim(), now_ms, user_id],
    )?;
    if changed != 1 {
        return Err(StorageError::not_found("account"));
    }
    account_profile(conn, user_id)
}

pub fn change_password(
    conn: &mut Connection,
    user_id: &str,
    current_access_token: &str,
    old_password: &str,
    new_password: &str,
    now_ms: i64,
) -> StorageResult<()> {
    validate_password(new_password)?;
    let new_hash = hash_password(new_password)?;
    let current_access_hash = token_hash(current_access_token);
    let tx = conn.transaction()?;
    let account = tx
        .query_row(
            "SELECT password_hash, password_scheme, status FROM user_accounts WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StorageError::not_found("account"))?;
    if account.2 != "active" || !verify_password(&account.0, &account.1, old_password)? {
        return Err(StorageError::not_found("account credentials"));
    }
    tx.execute(
        "UPDATE user_accounts SET password_hash = ?1, password_scheme = ?2, updated_at_ms = ?3
         WHERE user_id = ?4",
        params![new_hash, PASSWORD_SCHEME, now_ms, user_id],
    )?;
    tx.execute(
        "UPDATE account_sessions SET revoked_at_ms = ?1
         WHERE user_id = ?2 AND access_token_hash <> ?3 AND revoked_at_ms IS NULL",
        params![now_ms, user_id, current_access_hash],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn revoke_current_session(
    conn: &Connection,
    access_token: &str,
    now_ms: i64,
) -> StorageResult<()> {
    let changed = conn.execute(
        "UPDATE account_sessions SET revoked_at_ms = ?1
         WHERE access_token_hash = ?2 AND revoked_at_ms IS NULL",
        params![now_ms, token_hash(access_token)],
    )?;
    if changed != 1 {
        return Err(StorageError::not_found("account session"));
    }
    Ok(())
}

pub fn revoke_all_sessions(conn: &Connection, user_id: &str, now_ms: i64) -> StorageResult<()> {
    conn.execute(
        "UPDATE account_sessions SET revoked_at_ms = ?1
         WHERE user_id = ?2 AND revoked_at_ms IS NULL",
        params![now_ms, user_id],
    )?;
    Ok(())
}

pub fn delete_account(
    conn: &mut Connection,
    user_id: &str,
    now_ms: i64,
) -> StorageResult<Option<AvatarMetadata>> {
    let tx = conn.transaction()?;
    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM user_accounts WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .optional()?;
    if status.as_deref() != Some("active") {
        return Err(StorageError::not_found("account"));
    }
    let avatar = avatar_metadata_for_user(&tx, user_id)?;
    tx.execute(
        "DELETE FROM play_intent_votes WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM user_avatars WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM avatar_moderation WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.execute(
        "UPDATE user_accounts
         SET status = 'deleted', display_name = 'Deleted user', updated_at_ms = ?1
         WHERE user_id = ?2",
        params![now_ms, user_id],
    )?;
    tx.execute(
        "UPDATE account_sessions SET revoked_at_ms = COALESCE(revoked_at_ms, ?1)
         WHERE user_id = ?2",
        params![now_ms, user_id],
    )?;
    tx.commit()?;
    Ok(avatar)
}

pub fn set_avatar_metadata(
    conn: &mut Connection,
    user_id: &str,
    content_hash: &str,
    storage_key: &str,
    now_ms: i64,
) -> StorageResult<AvatarMetadata> {
    if content_hash.len() != 64 || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::validation("avatar content hash is invalid"));
    }
    if storage_key.is_empty() || storage_key.len() > 160 {
        return Err(StorageError::validation("avatar storage key is invalid"));
    }
    let tx = conn.transaction()?;
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1 AND status = 'active')",
        params![user_id],
        |row| row.get(0),
    )?;
    if !active {
        return Err(StorageError::not_found("account"));
    }
    let version: i64 = tx.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM user_avatars WHERE user_id = ?1",
        params![user_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO user_avatars (user_id, version, content_hash, media_type, storage_key, updated_at_ms)
         VALUES (?1, ?2, ?3, 'image/webp', ?4, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
             version = excluded.version,
             content_hash = excluded.content_hash,
             media_type = excluded.media_type,
             storage_key = excluded.storage_key,
             updated_at_ms = excluded.updated_at_ms",
        params![user_id, version, content_hash, storage_key, now_ms],
    )?;
    // A block is attached to the moderated content, not permanently to the
    // account. A genuinely new upload may be displayed while the audit trail
    // remains in audit_events.
    tx.execute(
        "DELETE FROM avatar_moderation WHERE user_id = ?1 AND content_hash <> ?2",
        params![user_id, content_hash],
    )?;
    let metadata =
        avatar_metadata_for_user(&tx, user_id)?.ok_or_else(|| StorageError::not_found("avatar"))?;
    tx.commit()?;
    Ok(metadata)
}

pub fn delete_avatar_metadata(
    conn: &mut Connection,
    user_id: &str,
) -> StorageResult<Option<AvatarMetadata>> {
    let tx = conn.transaction()?;
    let metadata = avatar_metadata_for_user(&tx, user_id)?;
    tx.execute(
        "DELETE FROM user_avatars WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM avatar_moderation WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.commit()?;
    Ok(metadata)
}

/// Block or unblock the account's current avatar. Blocks are content-hash
/// scoped: replacing the image naturally clears the active block, while the
/// immutable audit event keeps the moderator decision traceable.
pub fn set_avatar_moderation(
    conn: &mut Connection,
    user_id: &str,
    actor: &str,
    reason: &str,
    blocked: bool,
    now_ms: i64,
) -> StorageResult<()> {
    let actor = actor.trim();
    let reason = reason.trim();
    if actor.is_empty() || actor.chars().count() > 128 || actor.chars().any(char::is_control) {
        return Err(StorageError::validation("moderator identity is invalid"));
    }
    if reason.is_empty() || reason.chars().count() > 500 || reason.chars().any(char::is_control) {
        return Err(StorageError::validation("moderation reason is invalid"));
    }

    let tx = conn.transaction()?;
    let avatar =
        avatar_metadata_for_user(&tx, user_id)?.ok_or_else(|| StorageError::not_found("avatar"))?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT content_hash FROM avatar_moderation WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .optional()?;

    let changed = if blocked {
        if existing.as_deref() == Some(avatar.content_hash.as_str()) {
            false
        } else {
            tx.execute(
                "INSERT INTO avatar_moderation (user_id, content_hash, blocked_at_ms, blocked_by, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user_id) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    blocked_at_ms = excluded.blocked_at_ms,
                    blocked_by = excluded.blocked_by,
                    reason = excluded.reason",
                params![user_id, avatar.content_hash, now_ms, actor, reason],
            )?;
            true
        }
    } else if existing.is_some() {
        tx.execute(
            "DELETE FROM avatar_moderation WHERE user_id = ?1",
            params![user_id],
        )?;
        true
    } else {
        false
    };

    if changed {
        // Versioning makes existing avatar URLs cache-bust after a moderation
        // decision, and the M7 trigger invalidates community ETags.
        if blocked || existing.as_deref() == Some(avatar.content_hash.as_str()) {
            tx.execute(
                "UPDATE user_avatars SET version = version + 1, updated_at_ms = ?1
                 WHERE user_id = ?2 AND content_hash = ?3",
                params![now_ms, user_id, avatar.content_hash],
            )?;
        }
        let action = if blocked {
            "avatar_blocked"
        } else {
            "avatar_unblocked"
        };
        let before = format!(
            r#"{{"content_hash":"{}","blocked":{}}}"#,
            avatar.content_hash, !blocked
        );
        let after = format!(
            r#"{{"content_hash":"{}","blocked":{}}}"#,
            avatar.content_hash, blocked
        );
        tx.execute(
            "INSERT INTO audit_events (
                actor, action, entity_type, entity_key, before_json, after_json, reason, request_id, created_at_ms
             ) VALUES (?1, ?2, 'avatar', ?3, ?4, ?5, ?6, NULL, ?7)",
            params![actor, action, user_id, before, after, reason, now_ms],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn avatar_by_public_id(conn: &Connection, public_id: &str) -> StorageResult<AvatarLookup> {
    conn.query_row(
        "SELECT a.display_name, COALESCE(v.version, 0),
                CASE WHEN moderation.user_id IS NULL THEN v.storage_key ELSE NULL END,
                CASE WHEN moderation.user_id IS NULL THEN v.media_type ELSE NULL END
         FROM user_accounts AS a
         LEFT JOIN user_avatars AS v ON v.user_id = a.user_id
         LEFT JOIN avatar_moderation AS moderation
           ON moderation.user_id = a.user_id AND moderation.content_hash = v.content_hash
         WHERE a.avatar_public_id = ?1 AND a.status = 'active'",
        params![public_id],
        |row| {
            Ok(AvatarLookup {
                display_name: row.get(0)?,
                version: row.get::<_, i64>(1)?.max(0) as u32,
                storage_key: row.get(2)?,
                media_type: row.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| StorageError::not_found("avatar"))
}

pub fn account_ai_settings(conn: &Connection, user_id: &str) -> StorageResult<AiSettings> {
    let settings = conn
        .query_row(
            "SELECT mode, provider, base_url, model, encrypted_api_key, updated_at_ms
             FROM user_ai_credentials WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((mode, provider, base_url, model, encrypted_key, updated_at_ms)) = settings else {
        return Ok(AiSettings {
            mode: AiMode::Builtin,
            provider: None,
            base_url: None,
            model: None,
            configured: false,
            key_mask: None,
            updated_at_ms: None,
        });
    };
    let mode = AiMode::parse(&mode)
        .ok_or_else(|| StorageError::migration("unknown stored AI settings mode"))?;
    Ok(AiSettings {
        mode,
        provider,
        base_url,
        model,
        configured: encrypted_key.is_some(),
        // The true key is unavailable from this read path by design.
        key_mask: encrypted_key.as_ref().map(|_| "********".to_owned()),
        updated_at_ms: Some(updated_at_ms),
    })
}

pub fn put_account_ai_settings(
    conn: &mut Connection,
    user_id: &str,
    input: &PutAiSettings,
    _cipher: Option<&CredentialCipher>,
    now_ms: i64,
) -> StorageResult<AiSettings> {
    let tx = conn.transaction()?;
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1 AND status = 'active')",
        params![user_id],
        |row| row.get(0),
    )?;
    if !active {
        return Err(StorageError::not_found("account"));
    }
    match input.mode {
        AiMode::Builtin | AiMode::Off => {
            tx.execute(
                "INSERT INTO user_ai_credentials (
                    user_id, mode, provider, base_url, model, encrypted_api_key, key_version, updated_at_ms
                 ) VALUES (?1, ?2, NULL, NULL, NULL, NULL, NULL, ?3)
                 ON CONFLICT(user_id) DO UPDATE SET
                    mode = excluded.mode, provider = NULL, base_url = NULL, model = NULL,
                    encrypted_api_key = NULL, key_version = NULL, updated_at_ms = excluded.updated_at_ms",
                params![user_id, input.mode.as_str(), now_ms],
            )?;
        }
        AiMode::Custom => {
            let provider = input
                .provider
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| StorageError::validation("custom AI provider is required"))?;
            let base_url = input
                .base_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| StorageError::validation("custom AI base URL is required"))?;
            let model = input
                .model
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| StorageError::validation("custom AI model is required"))?;
            tx.execute(
                "INSERT INTO user_ai_credentials (
                    user_id, mode, provider, base_url, model, encrypted_api_key, key_version, updated_at_ms
                 ) VALUES (?1, 'custom', ?2, ?3, ?4, NULL, NULL, ?5)
                 ON CONFLICT(user_id) DO UPDATE SET
                    mode = 'custom', provider = excluded.provider, base_url = excluded.base_url,
                    model = excluded.model, encrypted_api_key = NULL, key_version = NULL,
                    updated_at_ms = excluded.updated_at_ms",
                params![user_id, provider, base_url, model, now_ms],
            )?;
        }
    }
    tx.commit()?;
    account_ai_settings(conn, user_id)
}

pub fn delete_custom_ai_key(
    conn: &mut Connection,
    user_id: &str,
    now_ms: i64,
) -> StorageResult<AiSettings> {
    let input = PutAiSettings {
        mode: AiMode::Builtin,
        provider: None,
        base_url: None,
        model: None,
        api_key: None,
    };
    put_account_ai_settings(conn, user_id, &input, None, now_ms)
}

/// Return the number of built-in AI requests accepted for an account on the
/// supplied UTC day. This deliberately exposes aggregate usage only.
pub fn account_ai_daily_usage(
    conn: &Connection,
    user_id: &str,
    day_utc: i64,
) -> StorageResult<u32> {
    let usage: Option<i64> = conn
        .query_row(
            "SELECT builtin_requests FROM account_ai_usage WHERE user_id = ?1 AND day_utc = ?2",
            params![user_id, day_utc],
            |row| row.get(0),
        )
        .optional()?;
    Ok(usage.unwrap_or(0).max(0).try_into().unwrap_or(u32::MAX))
}

/// Atomically reserve one built-in AI request if the account remains active
/// and has not exhausted its daily allowance. `None` means quota exhausted.
pub fn consume_account_ai_quota(
    conn: &mut Connection,
    user_id: &str,
    day_utc: i64,
    daily_limit: u32,
) -> StorageResult<Option<u32>> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1 AND status = 'active')",
        params![user_id],
        |row| row.get(0),
    )?;
    if !active {
        return Err(StorageError::not_found("account"));
    }
    let current: Option<i64> = transaction
        .query_row(
            "SELECT builtin_requests FROM account_ai_usage WHERE user_id = ?1 AND day_utc = ?2",
            params![user_id, day_utc],
            |row| row.get(0),
        )
        .optional()?;
    let current = current.unwrap_or(0).max(0);
    if current >= i64::from(daily_limit) {
        transaction.commit()?;
        return Ok(None);
    }
    let next = current.saturating_add(1);
    transaction.execute(
        "INSERT INTO account_ai_usage (user_id, day_utc, builtin_requests)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, day_utc) DO UPDATE SET builtin_requests = excluded.builtin_requests",
        params![user_id, day_utc, next],
    )?;
    transaction.commit()?;
    Ok(Some(next.try_into().unwrap_or(u32::MAX)))
}

pub fn custom_ai_credential(
    conn: &Connection,
    user_id: &str,
    cipher: Option<&CredentialCipher>,
) -> StorageResult<Option<CustomAiCredential>> {
    let stored = conn
        .query_row(
            "SELECT mode, provider, base_url, model, encrypted_api_key
             FROM user_ai_credentials WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((mode, provider, base_url, model, encrypted_key)) = stored else {
        return Ok(None);
    };
    if mode != AiMode::Custom.as_str() {
        return Ok(None);
    }
    let cipher = cipher.ok_or_else(|| {
        StorageError::validation(
            "AI custom configuration is unavailable: master key is not configured",
        )
    })?;
    let api_key =
        cipher
            .decrypt(&encrypted_key.ok_or_else(|| {
                StorageError::migration("custom AI setting has no encrypted key")
            })?)?;
    Ok(Some(CustomAiCredential {
        provider: provider
            .ok_or_else(|| StorageError::migration("custom AI setting has no provider"))?,
        base_url: base_url
            .ok_or_else(|| StorageError::migration("custom AI setting has no base URL"))?,
        model: model.ok_or_else(|| StorageError::migration("custom AI setting has no model"))?,
        api_key,
    }))
}

pub fn account_is_active(conn: &Connection, user_id: &str) -> StorageResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_accounts WHERE user_id = ?1 AND status = 'active')",
        params![user_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn create_account_session(
    conn: &Connection,
    user_id: &str,
    device_label: &str,
    now_ms: i64,
) -> StorageResult<AccountSessionTokens> {
    let session_id = format!("{SESSION_PREFIX}{}", random_hex::<16>()?);
    let access_token = format!("{ACCESS_PREFIX}{}", random_hex::<32>()?);
    let refresh_token = format!("{REFRESH_PREFIX}{}", random_hex::<32>()?);
    let expires_at_ms = now_ms.saturating_add(ACCESS_TOKEN_TTL_MS);
    let refresh_expires_at_ms = now_ms.saturating_add(REFRESH_TOKEN_TTL_MS);
    conn.execute(
        "INSERT INTO account_sessions (
            session_id, user_id, access_token_hash, refresh_token_hash, device_label,
            issued_at_ms, expires_at_ms, refresh_expires_at_ms, revoked_at_ms, replaced_by_session_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL)",
        params![
            session_id,
            user_id,
            token_hash(&access_token),
            token_hash(&refresh_token),
            device_label,
            now_ms,
            expires_at_ms,
            refresh_expires_at_ms
        ],
    )?;
    Ok(AccountSessionTokens {
        session_id,
        user_id: user_id.to_owned(),
        access_token,
        refresh_token,
        expires_at_ms,
        refresh_expires_at_ms,
    })
}

fn merge_anonymous_subject(
    conn: &Connection,
    source_user_id: &str,
    account_user_id: &str,
    preference_choice: Option<PreferenceChoice>,
) -> StorageResult<()> {
    merge_preferences(conn, source_user_id, account_user_id, preference_choice)?;
    // Feedback ids are global within one table. Removing destination-key
    // conflicts first lets us move the remaining rows in place, preserving
    // every feedback_id/undone_by relationship and its idempotent undo result.
    conn.execute(
        "DELETE FROM feedback_events
         WHERE user_id = ?2
           AND EXISTS (
               SELECT 1
               FROM feedback_events AS destination
               WHERE destination.user_id = ?1
                 AND destination.idempotency_key = feedback_events.idempotency_key
           )",
        params![account_user_id, source_user_id],
    )?;
    conn.execute(
        "UPDATE feedback_events SET user_id = ?1 WHERE user_id = ?2",
        params![account_user_id, source_user_id],
    )?;
    // The primary key on (app_id, user_id) is the account-level de-duplication
    // rule. Any source duplicate is discarded without adding public count.
    conn.execute(
        "INSERT OR IGNORE INTO play_intent_votes (app_id, user_id, created_at_ms)
         SELECT app_id, ?1, created_at_ms FROM play_intent_votes WHERE user_id = ?2",
        params![account_user_id, source_user_id],
    )?;
    conn.execute(
        "DELETE FROM play_intent_votes WHERE user_id = ?1",
        params![source_user_id],
    )?;
    Ok(())
}

fn merge_preferences(
    conn: &Connection,
    source_user_id: &str,
    account_user_id: &str,
    preference_choice: Option<PreferenceChoice>,
) -> StorageResult<()> {
    let source = load_preferences(conn, source_user_id)?;
    let destination = load_preferences(conn, account_user_id)?;
    let (source, destination) = match (source, destination) {
        (Some(source), Some(destination)) => (source, destination),
        (Some(_), None) => {
            conn.execute(
                "UPDATE user_preferences SET user_id = ?1 WHERE user_id = ?2",
                params![account_user_id, source_user_id],
            )?;
            return Ok(());
        }
        _ => return Ok(()),
    };
    let defaults = UserPreferences::default();
    let keep_source = if source == defaults && destination != defaults {
        false
    } else if source != defaults && destination == defaults {
        true
    } else if source == defaults && destination == defaults {
        false
    } else {
        match preference_choice {
            Some(PreferenceChoice::Anonymous) => true,
            Some(PreferenceChoice::Account) => false,
            None => return Err(StorageError::conflict("merge_preference_choice_required")),
        }
    };
    if keep_source {
        conn.execute(
            "DELETE FROM user_preferences WHERE user_id = ?1",
            params![account_user_id],
        )?;
        conn.execute(
            "UPDATE user_preferences SET user_id = ?1 WHERE user_id = ?2",
            params![account_user_id, source_user_id],
        )?;
    } else {
        conn.execute(
            "DELETE FROM user_preferences WHERE user_id = ?1",
            params![source_user_id],
        )?;
    }
    Ok(())
}

fn load_preferences(conn: &Connection, user_id: &str) -> StorageResult<Option<UserPreferences>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_preferences WHERE user_id = ?1)",
        params![user_id],
        |row| row.get(0),
    )?;
    if exists {
        crate::users::get_preferences(conn, user_id).map(Some)
    } else {
        Ok(None)
    }
}

fn create_anonymous_subject(conn: &Connection, now_ms: i64) -> StorageResult<String> {
    let user_id = format!("u_{}", random_hex::<16>()?);
    let access_token = format!("mpgs_at_{}", random_hex::<32>()?);
    let refresh_token = format!("mpgs_rt_{}", random_hex::<32>()?);
    conn.execute(
        "INSERT INTO anonymous_users (
            user_id, created_at_ms, last_active_at_ms, access_token_hash, refresh_token_hash,
            access_expires_at_ms, refresh_expires_at_ms
         ) VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)",
        params![
            user_id,
            now_ms,
            token_hash(&access_token),
            token_hash(&refresh_token),
            now_ms.saturating_add(ACCESS_TOKEN_TTL_MS),
            now_ms.saturating_add(REFRESH_TOKEN_TTL_MS)
        ],
    )?;
    insert_default_preferences(conn, &user_id, now_ms)?;
    Ok(user_id)
}

fn ensure_anonymous_subject(conn: &Connection, user_id: &str) -> StorageResult<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM anonymous_users WHERE user_id = ?1)",
        params![user_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StorageError::not_found("anonymous session"))
    }
}

fn insert_default_preferences(conn: &Connection, user_id: &str, now_ms: i64) -> StorageResult<()> {
    let prefs = UserPreferences::default();
    conn.execute(
        "INSERT INTO user_preferences (
            user_id, version, preference_confidence, party_size, coop_competitive,
            session_minutes_min, session_minutes_max,
            budget_currency, budget_max_each_minor, platforms_json, self_hosting_willingness,
            languages_json, excluded_modes_json, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            user_id,
            prefs.version,
            prefs.preference_confidence,
            prefs.party_size,
            prefs.coop_competitive,
            prefs.session_minutes_min,
            prefs.session_minutes_max,
            prefs.budget_currency,
            prefs.budget_max_each_minor,
            serde_json::to_string(&prefs.platforms)?,
            prefs.self_hosting_willingness,
            serde_json::to_string(&prefs.languages)?,
            serde_json::to_string(&prefs.excluded_modes)?,
            now_ms
        ],
    )?;
    Ok(())
}

fn avatar_metadata_for_user(
    conn: &Connection,
    user_id: &str,
) -> StorageResult<Option<AvatarMetadata>> {
    conn.query_row(
        "SELECT user_id, version, content_hash, media_type, storage_key, updated_at_ms
         FROM user_avatars WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(AvatarMetadata {
                user_id: row.get(0)?,
                version: row.get::<_, i64>(1)?.max(0) as u32,
                content_hash: row.get(2)?,
                media_type: row.get(3)?,
                storage_key: row.get(4)?,
                updated_at_ms: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn normalize_username(value: &str) -> StorageResult<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if !(3..=32).contains(&normalized.len())
        || !normalized
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(StorageError::validation(
            "username must contain 3 to 32 ASCII letters, digits, or underscores",
        ));
    }
    Ok(normalized)
}

fn validate_display_name(value: &str) -> StorageResult<()> {
    let value = value.trim();
    let count = value.chars().count();
    if !(1..=40).contains(&count) || value.chars().any(char::is_control) {
        return Err(StorageError::validation(
            "display_name must contain 1 to 40 non-control Unicode characters",
        ));
    }
    Ok(())
}

fn validate_password(value: &str) -> StorageResult<()> {
    let count = value.chars().count();
    if !(10..=128).contains(&count) {
        return Err(StorageError::validation(
            "password must contain 10 to 128 characters",
        ));
    }
    Ok(())
}

fn normalize_device_label(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "Unknown device".to_owned()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn hash_password(password: &str) -> StorageResult<String> {
    let mut salt_bytes = [0_u8; 16];
    getrandom::fill(&mut salt_bytes)
        .map_err(|error| StorageError::Io(std::io::Error::other(error.to_string())))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|_| StorageError::validation("unable to create password salt"))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| StorageError::validation("unable to hash password"))
}

fn verify_password(stored: &str, scheme: &str, password: &str) -> StorageResult<bool> {
    if scheme != PASSWORD_SCHEME {
        return Ok(false);
    }
    let parsed = match PasswordHash::new(stored) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

fn random_hex<const BYTES: usize>() -> StorageResult<String> {
    let mut bytes = [0_u8; BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| StorageError::Io(std::io::Error::other(error.to_string())))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hex_pair(raw: &str) -> Option<u8> {
    let mut bytes = raw.bytes();
    let high = bytes.next().and_then(hex_nibble)?;
    let low = bytes.next().and_then(hex_nibble)?;
    Some((high << 4) | low)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use mpgs_domain::FeedbackType;

    fn setup() -> (Database, String) {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| crate::migrate::migrate_to_latest(conn, 0))
            .unwrap();
        let anonymous = db
            .with_conn_mut(|conn| crate::users::create_anonymous_session(conn, 1))
            .unwrap();
        (db, anonymous.user_id)
    }

    #[test]
    fn account_passwords_are_argon2_and_sessions_rotate() {
        let (db, anonymous_id) = setup();
        let created = db
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: "Player_One".into(),
                        display_name: "Player One".into(),
                        password: "not-a-plain-password".into(),
                        device_label: "Test".into(),
                    },
                    Some(&anonymous_id),
                    10,
                )
            })
            .unwrap();
        assert_eq!(created.user_id, anonymous_id);
        assert!(
            db.with_conn(|conn| {
                let hash: String = conn.query_row(
                    "SELECT password_hash FROM user_accounts WHERE user_id = ?1",
                    params![created.user_id],
                    |row| row.get(0),
                )?;
                Ok::<_, StorageError>(hash.starts_with("$argon2id$"))
            })
            .unwrap()
        );
        let refreshed = db
            .with_conn_mut(|conn| refresh_account_session(conn, &created.refresh_token, 20))
            .unwrap();
        assert_ne!(created.refresh_token, refreshed.refresh_token);
        assert!(
            db.with_conn_mut(|conn| refresh_account_session(conn, &created.refresh_token, 30))
                .is_err()
        );
        assert!(
            db.with_conn(|conn| resolve_account_user_id(conn, &refreshed.access_token, 31))
                .is_err()
        );
    }

    #[test]
    fn dummy_login_hash_is_valid_and_never_authenticates() {
        assert!(
            !verify_password(DUMMY_PASSWORD_HASH, PASSWORD_SCHEME, "unknown-password").unwrap()
        );
    }

    #[test]
    fn account_merge_preserves_idempotent_feedback_undo_relationships() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let account = register_account(
                conn,
                &RegisterAccount {
                    username: "merge_target".into(),
                    display_name: "Merge Target".into(),
                    password: "target-password-long".into(),
                    device_label: "target".into(),
                },
                None,
                1,
            )?;
            let source = crate::users::create_anonymous_session(conn, 2)?;
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state, created_at_ms, updated_at_ms
                 ) VALUES (42, 'game', 'Merge Test', 'released', 1, 1)",
                [],
            )?;
            let fingerprint =
                crate::feedback::request_fingerprint(42, FeedbackType::Like, None, None)?;
            let original = crate::feedback::create_feedback(
                conn,
                &source.user_id,
                42,
                FeedbackType::Like,
                None,
                "source-like",
                None,
                &fingerprint,
                3,
            )?;
            let undo =
                crate::feedback::undo_feedback(conn, &source.user_id, original.feedback_id, 4)?;

            let logged_in = login_account(
                conn,
                &LoginAccount {
                    username: "merge_target".into(),
                    password: "target-password-long".into(),
                    device_label: "merged".into(),
                    preference_choice: None,
                },
                Some(&source.user_id),
                5,
            )?;
            assert_eq!(logged_in.user_id, account.user_id);

            let (moved_id, moved_undo): (i64, Option<i64>) = conn.query_row(
                "SELECT feedback_id, undone_by
                 FROM feedback_events
                 WHERE user_id = ?1 AND idempotency_key = 'source-like'",
                params![account.user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(moved_id, original.feedback_id);
            assert_eq!(moved_undo, Some(undo.feedback_id));
            let replay = crate::feedback::undo_feedback(conn, &account.user_id, moved_id, 6)?;
            assert_eq!(replay.feedback_id, undo.feedback_id);
            assert_eq!(replay.user_id, account.user_id);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn custom_ai_metadata_is_persisted_without_the_device_key() {
        let (db, anonymous_id) = setup();
        db.with_conn_mut(|conn| {
            register_account(
                conn,
                &RegisterAccount {
                    username: "player".into(),
                    display_name: "Player".into(),
                    password: "long-enough-password".into(),
                    device_label: "Test".into(),
                },
                Some(&anonymous_id),
                10,
            )
        })
        .unwrap();
        let settings = db
            .with_conn_mut(|conn| {
                put_account_ai_settings(
                    conn,
                    &anonymous_id,
                    &PutAiSettings {
                        mode: AiMode::Custom,
                        provider: Some("openai_compat".into()),
                        base_url: Some("https://provider.example/v1".into()),
                        model: Some("test-model".into()),
                        api_key: Some("secret-key".into()),
                    },
                    None,
                    20,
                )
            })
            .unwrap();
        assert_eq!(settings.mode, AiMode::Custom);
        assert!(!settings.configured);
        assert_eq!(settings.provider.as_deref(), Some("openai_compat"));
        let encrypted: Option<Vec<u8>> = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT encrypted_api_key FROM user_ai_credentials WHERE user_id = ?1",
                    params![anonymous_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert!(encrypted.is_none());
    }

    #[test]
    fn built_in_ai_quota_is_atomic_and_per_account() {
        let (db, anonymous_id) = setup();
        db.with_conn_mut(|conn| {
            register_account(
                conn,
                &RegisterAccount {
                    username: "quota_player".into(),
                    display_name: "Quota Player".into(),
                    password: "long-enough-password".into(),
                    device_label: "Test".into(),
                },
                Some(&anonymous_id),
                10,
            )
        })
        .unwrap();
        assert_eq!(
            db.with_conn_mut(|conn| consume_account_ai_quota(conn, &anonymous_id, 20, 2))
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            db.with_conn_mut(|conn| consume_account_ai_quota(conn, &anonymous_id, 20, 2))
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            db.with_conn_mut(|conn| consume_account_ai_quota(conn, &anonymous_id, 20, 2))
                .unwrap(),
            None
        );
        assert_eq!(
            db.with_conn(|conn| account_ai_daily_usage(conn, &anonymous_id, 20))
                .unwrap(),
            2
        );
    }

    #[test]
    fn file_backed_account_authentication_updates_activity_and_can_log_out() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path().join("accounts.db")).unwrap();
        db.migrate().unwrap();
        let now_ms = db.now_ms();
        let anonymous = db
            .with_conn_mut(|conn| crate::users::create_anonymous_session(conn, now_ms))
            .unwrap();
        let account = db
            .with_conn_mut(|conn| {
                register_account(
                    conn,
                    &RegisterAccount {
                        username: "file_account".into(),
                        display_name: "File Account".into(),
                        password: "long-enough-password".into(),
                        device_label: "Test".into(),
                    },
                    Some(&anonymous.user_id),
                    now_ms.saturating_add(1),
                )
            })
            .unwrap();
        let repo = crate::Repository::new(db);
        assert_eq!(
            repo.resolve_account_access_token(&account.access_token)
                .unwrap(),
            account.user_id
        );
        repo.update_account_display_name(&account.user_id, "Renamed")
            .unwrap();
        repo.revoke_current_account_session(&account.access_token)
            .unwrap();
        assert!(
            repo.resolve_account_access_token(&account.access_token)
                .is_err()
        );
    }

    #[test]
    fn avatar_moderation_hides_the_current_image_and_keeps_an_audit_event() {
        let (db, anonymous_id) = setup();
        db.with_conn_mut(|conn| {
            register_account(
                conn,
                &RegisterAccount {
                    username: "moderated_player".into(),
                    display_name: "Moderated Player".into(),
                    password: "long-enough-password".into(),
                    device_label: "Test".into(),
                },
                Some(&anonymous_id),
                10,
            )
        })
        .unwrap();
        let hash = "a".repeat(64);
        let before = db
            .with_conn_mut(|conn| {
                set_avatar_metadata(conn, &anonymous_id, &hash, "avatar-a.webp", 20)
            })
            .unwrap();
        let public_id = db
            .with_conn(|conn| account_profile(conn, &anonymous_id))
            .unwrap()
            .avatar_public_id;

        db.with_conn_mut(|conn| {
            set_avatar_moderation(
                conn,
                &anonymous_id,
                "moderator-1",
                "policy violation",
                true,
                30,
            )
        })
        .unwrap();
        let blocked = db
            .with_conn(|conn| avatar_by_public_id(conn, &public_id))
            .unwrap();
        assert!(blocked.storage_key.is_none());
        assert!(blocked.version > before.version);
        let audit: (String, String, String) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT action, reason, after_json FROM audit_events WHERE entity_key = ?1",
                    params![anonymous_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(audit.0, "avatar_blocked");
        assert_eq!(audit.1, "policy violation");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&audit.2).unwrap()["blocked"],
            true
        );

        db.with_conn_mut(|conn| {
            set_avatar_moderation(
                conn,
                &anonymous_id,
                "moderator-1",
                "appeal accepted",
                false,
                40,
            )
        })
        .unwrap();
        let unblocked = db
            .with_conn(|conn| avatar_by_public_id(conn, &public_id))
            .unwrap();
        assert_eq!(unblocked.storage_key.as_deref(), Some("avatar-a.webp"));
        assert!(unblocked.version > blocked.version);
    }
}
