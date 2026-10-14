//! AWS SigV4 query presigning for R2's S3 API, so Hanabi can upload originals
//! straight to R2 without the image bytes passing through (and billing CPU to)
//! this Worker.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub struct S3Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as SigV4 requires; `/` is kept only inside object paths.
pub fn uri_encode(input: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `2026-10-14T14:57:47.870Z` -> `20261014T145747Z`.
pub fn amz_date_from_iso(iso: &str) -> Option<String> {
    let digits: String = iso
        .get(..19)?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    (digits.len() == 14).then(|| format!("{}T{}Z", &digits[..8], &digits[8..]))
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Presigned URL for `method https://{host}{path}`. Every header in `headers`
/// is signed, so the uploader must send exactly these values.
#[allow(clippy::too_many_arguments)]
pub fn presign_url(
    creds: &S3Credentials,
    method: &str,
    host: &str,
    path: &str,
    region: &str,
    amz_date: &str,
    expires_secs: u32,
    headers: &[(&str, &str)],
) -> String {
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let mut signed: Vec<(String, String)> = std::iter::once(("host".to_string(), host.to_string()))
        .chain(
            headers
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string())),
        )
        .collect();
    signed.sort();
    let signed_names = signed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let mut query = [
        ("X-Amz-Algorithm", "AWS4-HMAC-SHA256".to_string()),
        (
            "X-Amz-Credential",
            format!("{}/{scope}", creds.access_key_id),
        ),
        ("X-Amz-Date", amz_date.to_string()),
        ("X-Amz-Expires", expires_secs.to_string()),
        ("X-Amz-SignedHeaders", signed_names.clone()),
    ];
    query.sort();
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_headers: String = signed
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let canonical_request = format!(
        "{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_names}\nUNSIGNED-PAYLOAD",
        uri_encode(path, true)
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let mut key = format!("AWS4{}", creds.secret_access_key).into_bytes();
    for part in [date, region, "s3", "aws4_request"] {
        key = hmac(&key, part);
    }
    let signature = hex::encode(hmac(&key, &string_to_sign));
    format!(
        "https://{host}{}?{canonical_query}&X-Amz-Signature={signature}",
        uri_encode(path, true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_aws_documented_presigned_get() {
        // https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html
        let creds = S3Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        };
        let url = presign_url(
            &creds,
            "GET",
            "examplebucket.s3.amazonaws.com",
            "/test.txt",
            "us-east-1",
            "20130524T000000Z",
            86400,
            &[],
        );
        assert!(url.ends_with(
            "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        ));
    }

    #[test]
    fn helpers() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(uri_encode("a b/c~", true), "a%20b/c~");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(
            amz_date_from_iso("2026-10-14T14:57:47.870Z").as_deref(),
            Some("20261014T145747Z")
        );
    }
}
