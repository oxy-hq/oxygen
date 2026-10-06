//! The client address the load balancer saw.
//!
//! Behind the load balancer the socket peer is the load balancer, so the
//! client is read from `X-Forwarded-For`. That header is a list each hop
//! appends to, and **the caller writes the front of it**: a request sent with
//! `X-Forwarded-For: 6.6.6.6` arrives as `6.6.6.6, <the real address>`. Only
//! the **last** entry is one our own load balancer wrote (the ALB targets the
//! pods directly and appends the address that connected to it). So that is the
//! entry every reader takes: audit rows, token usage and the public rate
//! limit. The first entry would let a caller choose what an audit row says, or
//! pick a fresh rate-limit bucket per request.
//!
//! Two limits:
//!
//! - With no load balancer in front (a dev box, a test) the whole header is
//!   the caller's, so there the address is a lead, not proof.
//! - This trusts exactly **one** hop. Put a CDN or a second proxy in front of
//!   the load balancer and the last entry becomes that proxy's address; this
//!   function is the one place to change then.

use axum::http::HeaderMap;

const HEADER: &str = "x-forwarded-for";

/// The bound on a stored address. An IPv6 address with a zone fits well
/// inside it.
pub const MAX_CLIENT_IP_CHARS: usize = 64;

/// The last `X-Forwarded-For` entry, trimmed and bounded: the address the load
/// balancer saw connect. `None` with no header, or when that entry is empty or
/// not text; an earlier entry is never promoted in its place.
///
/// A header sent on several lines is one list in line order, so the entry is
/// the last one of the last line.
pub fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(HEADER)
        .iter()
        .next_back()
        .and_then(|line| line.to_str().ok())
        .and_then(|line| line.rsplit(',').next())
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.chars().take(MAX_CLIENT_IP_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    /// A request carrying these `X-Forwarded-For` lines, in order.
    fn forwarded(lines: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append(HEADER, line.parse().unwrap());
        }
        headers
    }

    fn client(lines: &[&str]) -> Option<String> {
        client_ip(&forwarded(lines))
    }

    #[test]
    fn no_header_is_no_address() {
        assert_eq!(client_ip(&HeaderMap::new()), None);
    }

    #[test]
    fn a_single_hop_is_the_client() {
        assert_eq!(client(&["203.0.113.4"]).as_deref(), Some("203.0.113.4"));
    }

    #[test]
    fn several_hops_give_the_last() {
        assert_eq!(
            client(&["198.51.100.7, 10.0.0.2, 203.0.113.4"]).as_deref(),
            Some("203.0.113.4")
        );
    }

    #[test]
    fn a_spoofed_first_hop_is_ignored() {
        // What `X-Forwarded-For: 6.6.6.6` from the caller looks like once the
        // load balancer has appended the address it saw.
        assert_eq!(
            client(&["6.6.6.6, 203.0.113.4"]).as_deref(),
            Some("203.0.113.4")
        );
        // However many entries the caller wrote, and whatever they claim.
        assert_eq!(
            client(&["6.6.6.6, 127.0.0.1, 10.0.0.1, 203.0.113.4"]).as_deref(),
            Some("203.0.113.4")
        );
    }

    #[test]
    fn a_header_sent_on_several_lines_reads_as_one_list() {
        // `HeaderMap::get` would return the first line, which the caller wrote.
        assert_eq!(
            client(&["6.6.6.6, 7.7.7.7", "203.0.113.4"]).as_deref(),
            Some("203.0.113.4")
        );
        assert_eq!(
            client(&["6.6.6.6", "7.7.7.7, 203.0.113.4"]).as_deref(),
            Some("203.0.113.4")
        );
    }

    #[test]
    fn ipv6_is_kept_whole() {
        assert_eq!(client(&["2001:db8::1"]).as_deref(), Some("2001:db8::1"));
        assert_eq!(
            client(&["6.6.6.6, 2001:db8:85a3::8a2e:370:7334"]).as_deref(),
            Some("2001:db8:85a3::8a2e:370:7334")
        );
        assert_eq!(
            client(&["::1, fe80::1%eth0"]).as_deref(),
            Some("fe80::1%eth0")
        );
    }

    #[test]
    fn whitespace_around_an_entry_is_dropped() {
        assert_eq!(
            client(&["  6.6.6.6 ,\t 203.0.113.4  "]).as_deref(),
            Some("203.0.113.4")
        );
        assert_eq!(client(&[" 203.0.113.4"]).as_deref(), Some("203.0.113.4"));
    }

    #[test]
    fn an_empty_last_entry_is_no_address() {
        // Nothing a load balancer wrote. The entry before it is the caller's.
        assert_eq!(client(&["203.0.113.4, "]), None);
        assert_eq!(client(&["203.0.113.4", ""]), None);
        assert_eq!(client(&[" , "]), None);
        assert_eq!(client(&[""]), None);
    }

    #[test]
    fn an_entry_that_is_not_text_is_no_address() {
        let mut headers = forwarded(&["203.0.113.4"]);
        headers.append(HEADER, HeaderValue::from_bytes(b"\xff\xfe").unwrap());
        assert_eq!(client_ip(&headers), None);
    }

    #[test]
    fn a_long_entry_is_bounded() {
        let long = "9".repeat(4000);
        assert_eq!(client(&[long.as_str()]).unwrap().len(), MAX_CLIENT_IP_CHARS);
        // The bound is taken from the last entry, not from the header's start.
        let spoofed = format!("6.6.6.6, {long}");
        assert_eq!(
            client(&[spoofed.as_str()]).unwrap(),
            "9".repeat(MAX_CLIENT_IP_CHARS)
        );
        assert_eq!(MAX_CLIENT_IP_CHARS, 64);
    }
}
