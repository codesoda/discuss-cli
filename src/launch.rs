use std::io::{self, Write};
use std::net::SocketAddr;

pub trait BrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBrowserLauncher;

impl BrowserLauncher for SystemBrowserLauncher {
    fn open(&self, url: &str) -> io::Result<()> {
        open::that(url).map(|_| ()).map_err(io::Error::other)
    }
}

pub fn loopback_url(addr: SocketAddr) -> String {
    format!("http://127.0.0.1:{}", addr.port())
}

pub fn announce_listening<W, L>(
    writer: &mut W,
    launcher: &L,
    url: &str,
    auto_open: bool,
) -> io::Result<()>
where
    W: Write,
    L: BrowserLauncher,
{
    writeln!(writer, "listening on {url}")?;

    if auto_open {
        if let Err(error) = launcher.open(url) {
            tracing::warn!(%url, error = %error, "failed to open browser");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;

    #[derive(Debug, Default)]
    struct FakeLauncher {
        opened_urls: RefCell<Vec<String>>,
        fail: bool,
    }

    impl BrowserLauncher for FakeLauncher {
        fn open(&self, url: &str) -> io::Result<()> {
            self.opened_urls.borrow_mut().push(url.to_string());

            if self.fail {
                Err(io::Error::other("browser unavailable"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn loopback_url_uses_localhost_and_port() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 8888));

        assert_eq!(loopback_url(addr), "http://127.0.0.1:8888");
    }

    #[test]
    fn announce_listening_writes_exact_url_line_and_opens_when_enabled() {
        let launcher = FakeLauncher::default();
        let mut stderr = Vec::new();

        announce_listening(&mut stderr, &launcher, "http://127.0.0.1:7777", true)
            .expect("announcement should succeed");

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be utf-8"),
            "listening on http://127.0.0.1:7777\n"
        );
        assert_eq!(
            launcher.opened_urls.borrow().as_slice(),
            ["http://127.0.0.1:7777"]
        );
    }

    #[test]
    fn announce_listening_suppresses_browser_open_when_disabled() {
        let launcher = FakeLauncher::default();
        let mut stderr = Vec::new();

        announce_listening(&mut stderr, &launcher, "http://127.0.0.1:7777", false)
            .expect("announcement should succeed");

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be utf-8"),
            "listening on http://127.0.0.1:7777\n"
        );
        assert!(launcher.opened_urls.borrow().is_empty());
    }

    #[test]
    fn browser_open_failure_does_not_fail_announcement() {
        let launcher = FakeLauncher {
            opened_urls: RefCell::new(Vec::new()),
            fail: true,
        };
        let mut stderr = Vec::new();

        announce_listening(&mut stderr, &launcher, "http://127.0.0.1:7777", true)
            .expect("browser failure should only be logged");

        assert_eq!(
            launcher.opened_urls.borrow().as_slice(),
            ["http://127.0.0.1:7777"]
        );
    }
}
