use anyhow::{Context, Result};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

pub(super) fn default_pipe_name() -> String {
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("AppData/Local"))
        .join("lean-ctx");
    let seed = format!("{username}:{}", data_dir.display());
    let hash = blake3::hash(seed.as_bytes());
    let short = &hash.to_hex()[..16];
    format!(r"\\.\pipe\lean-ctx-{short}")
}

pub(super) fn pipe_exists(name: &str) -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let wide: Vec<u16> = OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { WaitNamedPipeW(wide.as_ptr(), 1) != 0 }
}

/// Retries indefinitely on `NotFound` / `ERROR_PIPE_BUSY` (transient
/// conditions during named-pipe instance rotation). Callers should wrap
/// with `tokio::time::timeout` to prevent unbounded waits.
pub(super) async fn connect(
    pipe_name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return Ok(client),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => {
                anyhow::bail!("connect to daemon pipe {pipe_name}: {e}");
            }
        }
    }
}

/// Server-side named-pipe listener, analogous to `UnixListener`.
///
/// Each call to [`accept_pipe`] waits for a client to connect, hands back
/// the connected pipe, and creates a fresh instance for the next client.
pub struct NamedPipeListener {
    current: NamedPipeServer,
    name: String,
}

impl NamedPipeListener {
    pub fn bind(name: &str) -> Result<Self> {
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(name)
            .with_context(|| format!("bind named pipe {name}"))?;
        Ok(Self {
            current: server,
            name: name.to_string(),
        })
    }

    /// Wait for a client, return the connected pipe, prepare the next instance.
    pub async fn accept_pipe(&mut self) -> std::io::Result<NamedPipeServer> {
        self.current.connect().await?;
        let next = ServerOptions::new()
            .first_pipe_instance(false)
            .create(&self.name)?;
        Ok(std::mem::replace(&mut self.current, next))
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    /// connect() retries NotFound, does not return a hard error before test timeout.
    #[tokio::test]
    async fn connect_retries_notfound() {
        let pipe_name = r"\\.\pipe\lean-ctx-test-notfound";
        let result = tokio::time::timeout(Duration::from_millis(150), connect(pipe_name)).await;
        // Must time out (retries), NOT return a hard error.
        assert!(
            result.is_err(),
            "should retry and be killed by timeout, not hard-error"
        );
    }

    // TODO: connect_retries_pipe_busy — needs reliable ERROR_PIPE_BUSY reproduction.
    // PIPE_BUSY occurs when a pipe instance exists but all instances are busy (real
    // mid-rotation race). Constructing this in a unit test is fragile; verify on
    // Windows manually or via integration test with concurrent clients.

    /// connect() retries when an invalid pipe path produces NotFound on Windows.
    /// A bare filename (without \\.\pipe\ prefix) triggers ERROR_FILE_NOT_FOUND
    /// via CreateFileW, which maps to NotFound → hits the retry loop.
    /// Timeout proves it doesn't succeed from a malformed path.
    #[tokio::test]
    async fn connect_retries_notfound_on_invalid_format() {
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            connect("invalid_pipe_format_no_backslash_prefix"),
        )
        .await;
        assert!(
            result.is_err(),
            "should not succeed (NotFound → retry → timeout)"
        );
    }

    /// WaitNamedPipeW returns true for an existing pipe.
    #[test]
    fn pipe_exists_true() {
        let pipe_name = r"\\.\pipe\lean-ctx-test-exists";
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(pipe_name)
            .expect("create test pipe");
        assert!(pipe_exists(pipe_name));
    }

    /// WaitNamedPipeW returns false for a nonexistent pipe.
    #[test]
    fn pipe_exists_false() {
        let pipe_name = r"\\.\pipe\lean-ctx-test-gone-bogus";
        assert!(!pipe_exists(pipe_name));
    }

    /// accept_pipe() waits for current, creates next, returns connected server.
    /// Two sequential client→accept→drop cycles verify instance rotation.
    #[tokio::test]
    async fn accept_pipe_rotates_after_connect() {
        let pipe_name = r"\\.\pipe\lean-ctx-test-rotate";
        let mut listener = NamedPipeListener::bind(pipe_name).expect("bind");

        // Client 1
        let c1 = tokio::spawn({
            let n = pipe_name.to_string();
            async move { ClientOptions::new().open(&n) }
        });
        let s1 = listener.accept_pipe().await.expect("accept 1");
        let _c1 = c1.await.unwrap().expect("client 1 connect");
        drop(s1);

        // Client 2 — the rotated instance should be ready.
        let c2 = tokio::spawn({
            let n = pipe_name.to_string();
            async move { ClientOptions::new().open(&n) }
        });
        let s2 = listener.accept_pipe().await.expect("accept 2");
        let _c2 = c2.await.unwrap().expect("client 2 connect");
        drop(s2);
    }
}
