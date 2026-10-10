//! `Accept-Encoding` negotiation for the precompressed PWA build (`.br`/`.gz`
//! siblings written by `precompress()` in `web/packages/ui/src/vite`).

/// A content coding the web build may have precompressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    Brotli,
    Gzip,
}

impl Coding {
    pub fn token(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    /// The sibling file's extension (`app.js` → `app.js.br`).
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gz",
        }
    }
}

/// A qvalue in thousandths (`1`, `0.5`, `0.125`); anything else is malformed.
fn qvalue(text: &str) -> Option<u16> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = fraction.len() <= 3 && fraction.bytes().all(|byte| byte.is_ascii_digit());
    match whole {
        "1" if digits && fraction.bytes().all(|byte| byte == b'0') => Some(1000),
        "0" if digits => Some(format!("{fraction:0<3}").parse().ok()?),
        _ => None,
    }
}

/// The precompressed codings worth trying, best first. Each must be accepted
/// (q > 0, by name or through `*`) and rank at least as high as identity; ties
/// prefer br, then gzip, then identity. No header means identity only.
pub fn preferred(accept: Option<&str>) -> Vec<Coding> {
    let Some(accept) = accept else {
        return Vec::new();
    };
    let (mut br, mut gzip, mut identity, mut any) = (None, None, None, None);
    for element in accept.split(',') {
        let mut parts = element.split(';');
        let coding = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let mut q = Some(1000);
        for param in parts {
            if let Some((name, value)) = param.split_once('=')
                && name.trim().eq_ignore_ascii_case("q")
            {
                q = qvalue(value.trim());
            }
        }
        // A malformed qvalue drops the element rather than guessing.
        let Some(q) = q else { continue };
        match coding.as_str() {
            "br" => br = Some(q),
            "gzip" | "x-gzip" => gzip = Some(q),
            "identity" => identity = Some(q),
            "*" => any = Some(q),
            _ => {}
        }
    }
    // Unlisted identity only ranks last; listed (or through `*`) it competes by weight.
    // Excluded with q=0 it is still the last resort when no sibling exists: a 406 helps nobody.
    let identity = identity.or(any).unwrap_or(0);
    let mut ranked: Vec<(Coding, u16)> = [
        (Coding::Brotli, br.or(any).unwrap_or(0)),
        (Coding::Gzip, gzip.or(any).unwrap_or(0)),
    ]
    .into_iter()
    .filter(|&(_, q)| q > 0 && q >= identity)
    .collect();
    // Stable: equal weights keep br before gzip.
    ranked.sort_by_key(|&(_, q)| std::cmp::Reverse(q));
    ranked.into_iter().map(|(coding, _)| coding).collect()
}

#[cfg(test)]
mod tests {
    use super::{Coding::*, preferred, qvalue};

    #[test]
    fn qvalues_follow_the_rfc_grammar() {
        for (text, expected) in [
            ("1", Some(1000)),
            ("1.000", Some(1000)),
            ("0", Some(0)),
            ("0.5", Some(500)),
            ("0.125", Some(125)),
            ("0.", Some(0)),
            ("1.5", None),
            ("0.1234", None),
            ("2", None),
            ("-0", None),
            ("nan", None),
            ("", None),
        ] {
            assert_eq!(qvalue(text), expected, "{text}");
        }
    }

    #[test]
    fn br_then_gzip_unless_weights_or_exclusions_say_otherwise() {
        for (header, expected) in [
            (None, vec![]),
            (Some(""), vec![]),
            (Some("gzip, deflate, br, zstd"), vec![Brotli, Gzip]),
            (Some("gzip"), vec![Gzip]),
            (Some("br"), vec![Brotli]),
            (Some("BR;Q=1, GZip"), vec![Brotli, Gzip]),
            (Some("x-gzip"), vec![Gzip]),
            (Some("identity"), vec![]),
            (Some("br;q=0, gzip"), vec![Gzip]),
            (Some("br;q=0, gzip;q=0"), vec![]),
            (Some("br;q=0.4, gzip;q=0.8"), vec![Gzip, Brotli]),
            (Some("*"), vec![Brotli, Gzip]),
            (Some("*;q=0"), vec![]),
            (Some("*;q=0, gzip"), vec![Gzip]),
            (Some("br, *;q=0"), vec![Brotli]),
            (Some("identity;q=1, br;q=0.5"), vec![]),
            (Some("identity;q=0, gzip;q=0.1"), vec![Gzip]),
            (Some("br;q=bogus, gzip"), vec![Gzip]),
            (Some("deflate, zstd"), vec![]),
        ] {
            assert_eq!(preferred(header), expected, "{header:?}");
        }
    }
}
