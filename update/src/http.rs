//! Минимальный HTTPS GET через WinHTTP (системные прокси и хранилище сертификатов).

#[cfg(not(any(windows, switchdesk_check)))]
pub fn get(_url: &str, _max_bytes: usize) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "обновление работает только в Windows"))
}

#[cfg(any(windows, switchdesk_check))]
pub use win::get;

#[cfg(any(windows, switchdesk_check))]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::ptr;

    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
        WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE,
    };

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn check(ok: i32, what: &str) -> io::Result<()> {
        if ok == 0 {
            let e = io::Error::last_os_error();
            Err(io::Error::new(e.kind(), format!("{what}: {e}")))
        } else {
            Ok(())
        }
    }

    fn handle(h: *mut c_void, what: &str) -> io::Result<Handle> {
        if h.is_null() {
            let e = io::Error::last_os_error();
            Err(io::Error::new(e.kind(), format!("{what}: {e}")))
        } else {
            Ok(Handle(h))
        }
    }

    /// Разбирает `https://host[:port]/path`. Другие схемы не принимаются.
    pub(super) fn split_url(url: &str) -> Option<(&str, u16, &str)> {
        let rest = url.strip_prefix("https://")?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (authority, 443),
        };
        if host.is_empty() {
            return None;
        }
        Some((host, port, path))
    }

    pub fn get(url: &str, max_bytes: usize) -> io::Result<Vec<u8>> {
        let (host, port, path) = split_url(url)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "нужен https-адрес"))?;
        unsafe {
            let agent = wide(&format!("switchdesk/{}", crate::CURRENT));
            let session = handle(
                WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, ptr::null(), ptr::null(), 0),
                "WinHttpOpen",
            )?;
            WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000);
            let host_w = wide(host);
            let conn = handle(WinHttpConnect(session.0, host_w.as_ptr(), port, 0), "WinHttpConnect")?;
            let verb = wide("GET");
            let path_w = wide(path);
            let req = handle(
                WinHttpOpenRequest(
                    conn.0,
                    verb.as_ptr(),
                    path_w.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    WINHTTP_FLAG_SECURE,
                ),
                "WinHttpOpenRequest",
            )?;
            let headers = wide("Accept: application/vnd.github+json, application/octet-stream\r\n");
            check(
                WinHttpSendRequest(req.0, headers.as_ptr(), u32::MAX, ptr::null(), 0, 0, 0),
                "WinHttpSendRequest",
            )?;
            check(WinHttpReceiveResponse(req.0, ptr::null_mut()), "WinHttpReceiveResponse")?;

            let mut status: u32 = 0;
            let mut size = 4u32;
            check(
                WinHttpQueryHeaders(
                    req.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    ptr::null(),
                    (&mut status as *mut u32).cast(),
                    &mut size,
                    ptr::null_mut(),
                ),
                "WinHttpQueryHeaders",
            )?;
            if status != 200 {
                return Err(io::Error::other(format!("HTTP {status}")));
            }

            let mut out = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut n = 0u32;
                check(WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n), "WinHttpReadData")?;
                if n == 0 {
                    return Ok(out);
                }
                out.extend_from_slice(&buf[..n as usize]);
                if out.len() > max_bytes {
                    return Err(io::Error::other("ответ слишком большой"));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::split_url;

        #[test]
        fn splits_urls() {
            assert_eq!(split_url("https://api.github.com/repos/x"), Some(("api.github.com", 443, "/repos/x")));
            assert_eq!(split_url("https://h:8443"), Some(("h", 8443, "/")));
            assert_eq!(split_url("http://h/x"), None);
            assert_eq!(split_url("https:///x"), None);
        }
    }
}
