// Native librespot sign-in.
//
// The Web API sign-in uses the owner's Spotify app Client ID. That token can
// read and control playback, but Spotify refuses to mint reusable credentials
// for it, so librespot cannot use keymaster and cannot become a Connect device.
//
// librespot solves this with Spotify's public keymaster client ID and a
// loopback OAuth callback. The resulting session receives a reusable
// username/blob pair; that pair is what librespot needs for Spirc and for the
// audio key exchange. The blob is sealed with DPAPI and never written plainly.
use std::path::PathBuf;
use std::time::Duration;

use librespot::core::authentication::Credentials;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session;
use librespot::oauth::OAuthClientBuilder;
use librespot::protocol::authentication::AuthenticationType;

use crate::connect;
use crate::profile::Profile;

/// Spotify's public desktop/keymaster client. librespot itself uses this ID.
pub const KEYMASTER_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
pub const REDIRECT_PORT: u16 = 8898;

const SCOPES: &[&str] = &["streaming", "user-read-email", "user-read-private"];

const MESSAGE: &str = r#"
<!doctype html>
<html>
  <body style="font-family: sans-serif; background:#121212; color:#eee; padding:3rem">
    <h2>Rustify is connected</h2>
    <p>You can close this tab and return to Rustify.</p>
  </body>
</html>
"#;

pub fn default_path() -> PathBuf {
    Profile::default_path().with_file_name("native.bin")
}

/// Opens Spotify's consent page, waits for the loopback callback, exchanges the
/// code, then opens a librespot session to obtain reusable credentials.
pub fn login() -> Result<String, String> {
    let redirect = format!("http://127.0.0.1:{REDIRECT_PORT}/login");
    let client = OAuthClientBuilder::new(KEYMASTER_CLIENT_ID, &redirect, SCOPES.to_vec())
        .open_in_browser()
        .with_custom_message(MESSAGE)
        .build()
        .map_err(|error| {
            format!(
                "Could not start native Spotify sign-in: {error}. Check that port {REDIRECT_PORT} is free."
            )
        })?;
    let token = client
        .get_access_token()
        .map_err(|error| format!("Native Spotify sign-in failed: {error}"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the native auth runtime: {error}"))?;
    runtime.block_on(async move {
        let session = Session::new(SessionConfig::default(), None);
        session
            .connect(Credentials::with_access_token(token.access_token), false)
            .await
            .map_err(|error| format!("Native Spotify session failed: {error}"))?;

        let mut product = None;
        for _ in 0..50 {
            if let Some(kind) = session.get_user_attribute("type") {
                product = Some(kind);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        match product.as_deref() {
            Some("premium") => {}
            Some(other) => {
                return Err(format!(
                    "Spotify reports this account as {other}; full songs need Premium."
                ));
            }
            None => {
                return Err(
                    "Spotify did not confirm the account type; native sign-in was cancelled."
                        .to_string(),
                );
            }
        }

        let credentials = Credentials {
            username: Some(session.username()),
            auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: session.auth_data(),
        };
        if !save(&credentials) {
            return Err(
                "Spotify signed in, but the native credential could not be sealed with Windows DPAPI; refusing to cache it as plain text."
                    .to_string(),
            );
        }
        Ok(format!(
            "native_login=ok user={} credentials_saved=true path={}",
            session.username(),
            default_path().display()
        ))
    })
}

pub fn save(credentials: &Credentials) -> bool {
    let Ok(plain) = serde_json::to_vec(credentials) else {
        return false;
    };
    let Some(sealed) = connect::protect(&plain) else {
        return false;
    };
    let path = default_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, sealed).is_ok()
}

pub fn load() -> Option<Credentials> {
    let sealed = std::fs::read(default_path()).ok()?;
    let plain = connect::unprotect(&sealed)?;
    serde_json::from_slice(&plain).ok()
}

pub fn forget() {
    let _ = std::fs::remove_file(default_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusable_credentials_round_trip_through_the_sealed_store() {
        let dir = std::env::temp_dir().join(format!(
            "rustify-native-auth-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let credentials = Credentials {
            username: Some("owner".to_string()),
            auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: vec![1, 2, 3, 4],
        };
        let plain = serde_json::to_vec(&credentials).unwrap();
        // Exercise the same sealing helpers without touching the real profile.
        let sealed = connect::protect(&plain).expect("DPAPI available");
        assert_ne!(sealed, plain);
        let opened = connect::unprotect(&sealed).expect("unseal");
        let restored: Credentials = serde_json::from_slice(&opened).unwrap();
        assert_eq!(restored, credentials);
        let _ = std::fs::remove_dir_all(dir);
    }
}
