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

pub fn announce_endpoints<W, L>(
    writer: &mut W,
    launcher: &L,
    review_url: &str,
    proxy_url: Option<&str>,
    auto_open: bool,
) -> io::Result<()>
where
    W: Write,
    L: BrowserLauncher,
{
    writeln!(writer, "review UI/API: {review_url}")?;
    if let Some(proxy_url) = proxy_url {
        writeln!(writer, "website proxy: {proxy_url}")?;
    }

    if auto_open && let Err(error) = launcher.open(review_url) {
        tracing::warn!(url = %review_url, error = %error, "failed to open browser");
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
    fn announce_endpoints_writes_review_line_and_opens_browser() {
        let launcher = FakeLauncher::default();
        let mut stderr = Vec::new();

        announce_endpoints(&mut stderr, &launcher, "http://127.0.0.1:7777", None, true)
            .expect("announcement should succeed");

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be utf-8"),
            "review UI/API: http://127.0.0.1:7777\n"
        );
        assert_eq!(
            launcher.opened_urls.borrow().as_slice(),
            ["http://127.0.0.1:7777"]
        );
    }

    #[test]
    fn announce_endpoints_writes_proxy_line_when_present() {
        let launcher = FakeLauncher::default();
        let mut stderr = Vec::new();

        announce_endpoints(
            &mut stderr,
            &launcher,
            "http://127.0.0.1:7777",
            Some("http://127.0.0.1:8888"),
            false,
        )
        .expect("announcement should succeed");

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be utf-8"),
            "review UI/API: http://127.0.0.1:7777\nwebsite proxy: http://127.0.0.1:8888\n"
        );
    }

    #[test]
    fn announce_endpoints_suppresses_browser_open_when_disabled() {
        let launcher = FakeLauncher::default();
        let mut stderr = Vec::new();

        announce_endpoints(&mut stderr, &launcher, "http://127.0.0.1:7777", None, false)
            .expect("announcement should succeed");

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be utf-8"),
            "review UI/API: http://127.0.0.1:7777\n"
        );
        assert!(launcher.opened_urls.borrow().is_empty());
    }

    #[test]
    fn announce_endpoints_browser_failure_does_not_fail_announcement() {
        let launcher = FakeLauncher {
            opened_urls: RefCell::new(Vec::new()),
            fail: true,
        };
        let mut stderr = Vec::new();

        announce_endpoints(&mut stderr, &launcher, "http://127.0.0.1:7777", None, true)
            .expect("browser failure should only be logged");

        assert_eq!(
            launcher.opened_urls.borrow().as_slice(),
            ["http://127.0.0.1:7777"]
        );
    }
}
