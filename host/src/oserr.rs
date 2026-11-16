// FreeForGem — see LICENSE
//! Windows: the text of an OS error ("... (os error 997)") comes from the system in the language of the Windows installation and
//! reached the log / a console in another code page as unreadable characters.  The process asks for English system messages
//! (SetProcessPreferredUILanguages: FormatMessage, which the standard library uses for `io::Error`, then takes en-US; Windows
//! always has the en-US message text as a fallback).  FFG_OS_LANG=system keeps the language of the system.  Elsewhere: nothing.

/// Call first thing in main().
pub fn english() {
    #[cfg(windows)]
    {
        if std::env::var("FFG_OS_LANG").is_ok_and(|v| v.eq_ignore_ascii_case("system")) {
            return;
        }
        unsafe extern "system" {
            fn SetProcessPreferredUILanguages(flags: u32, langs: *const u16, num: *mut u32) -> i32;
        }
        const MUI_LANGUAGE_NAME: u32 = 0x8;
        let langs: Vec<u16> = "en-US\0\0".encode_utf16().collect(); // a double-null-terminated list
        let mut n = 0u32;
        // a failure leaves the system language: only the cosmetics are lost
        unsafe { SetProcessPreferredUILanguages(MUI_LANGUAGE_NAME, langs.as_ptr(), &mut n) };
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn os_error_text_is_english() {
        super::english();
        for code in [997, 10054, 10060] {
            let t = std::io::Error::from_raw_os_error(code).to_string();
            assert!(t.is_ascii(), "{code}: {t}");
            assert!(t.contains(&format!("os error {code}")), "{t}");
        }
    }
}
