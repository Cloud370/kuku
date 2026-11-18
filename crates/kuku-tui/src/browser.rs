use std::io;

use url::Url;

pub(crate) fn is_http_url(url: &str) -> bool {
    let Some((scheme, address)) = url.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !address.starts_with('/')
        && !url.contains(['\\', '"', '<', '>'])
        && !url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        && Url::parse(url).is_ok_and(|parsed| parsed.has_host())
}

pub(super) fn open_with(url: &str, opener: impl FnOnce(&str) -> io::Result<()>) -> io::Result<()> {
    if !is_http_url(url) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an HTTP or HTTPS URL",
        ));
    }
    opener(url).map_err(|error| io::Error::new(error.kind(), "Could not launch the system browser"))
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::open_with;

    #[test]
    fn browser_launcher_passes_the_exact_destination_and_blocks_other_schemes() {
        let destination = "HTTPS://example.test/a%2Fb?q=甲&next=%2f#章节";
        open_with(destination, |url| {
            assert_eq!(destination, url);
            Ok(())
        })
        .unwrap();

        for invalid in [
            "file:///private/example.txt",
            "javascript:void(0)",
            "data:text/plain,abc",
            "mailto:test@example.test",
            "//example.test",
            "https:///example.test",
            "https://",
            "https://example.test/\nmore",
        ] {
            let error = open_with(invalid, |_| {
                panic!("invalid destination reached the browser")
            })
            .unwrap_err();
            assert_eq!(io::ErrorKind::InvalidInput, error.kind());
        }
    }
}
