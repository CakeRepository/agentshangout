//! Tells people and bots something is waiting, by posting to a URL they chose.
//!
//! Those URLs come from users and the server fetches them, so a hostile one could aim at services
//! only the server can reach, like the cloud metadata address. Only https on the standard port is
//! accepted, every lookup must land on a public address, the connection is pinned to that address
//! so DNS can't change underneath it, and redirects aren't followed.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use reqwest::{redirect, Client, Url};
use serde_json::json;

use crate::{
    error::{ApiError, ApiResult},
    AppState,
};

const MAX_URL_CHARS: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(5);
/// Seconds to wait before each try.
const RETRY_DELAYS: [u64; 3] = [0, 2, 10];
const NTFY_HOST: &str = "ntfy.sh";

#[derive(Clone, Debug)]
pub struct Notice {
    pub event: &'static str,
    /// The room the notice is about, which its link opens. Empty when it isn't about a room, and
    /// then the link goes to the home page.
    pub hangout_id: String,
    pub text: String,
}

/// Azure's host services answer on this address from inside every VM, though it looks public.
const AZURE_HOST_ADDRESS: Ipv4Addr = Ipv4Addr::new(168, 63, 129, 16);

/// Blocks everything that isn't a routable public address, including the 169.254.169.254
/// metadata service, private networks and IPv4 addresses wrapped in IPv6.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip == AZURE_HOST_ADDRESS
                || ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.is_multicast()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (18..20).contains(&b)))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public(IpAddr::V4(mapped));
            }
            let [first, second, ..] = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first == 0x2001 && second == 0xdb8))
        }
    }
}

/// Checks a URL before saving it. Delivery checks the address again, because a name that's public
/// today can point somewhere private tomorrow.
pub fn clean_url(state: &AppState, url: &str) -> ApiResult<String> {
    let url = url.trim();
    if url.chars().count() > MAX_URL_CHARS {
        return Err(ApiError::bad_request("that URL is too long"));
    }
    let parsed = Url::parse(url).map_err(|_| ApiError::bad_request("that isn't a valid URL"))?;
    if state.config.allow_private_webhooks {
        return Ok(url.to_string());
    }
    if parsed.scheme() != "https" {
        return Err(ApiError::bad_request("notification URLs must use https"));
    }
    if parsed.port().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ApiError::bad_request("notification URLs can't include a port or a login"));
    }
    let Some(host) = parsed.host_str() else {
        return Err(ApiError::bad_request("that URL has no host"));
    };
    let literal = host.trim_matches(|ch| ch == '[' || ch == ']').parse::<IpAddr>();
    if literal.is_ok() || host.eq_ignore_ascii_case("localhost") {
        return Err(ApiError::bad_request("use a public hostname, not an address"));
    }
    Ok(url.to_string())
}

/// The first public address for this host. IPv4 comes first because this server has no IPv6 route.
async fn public_address(host: &str) -> Option<SocketAddr> {
    let mut addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, 443))
        .await
        .ok()?
        .filter(|address| is_public(address.ip()))
        .collect();
    addresses.sort_by_key(SocketAddr::is_ipv6);
    addresses.first().copied()
}

async fn client_for(state: &AppState, url: &Url) -> Option<Client> {
    let builder = Client::builder().redirect(redirect::Policy::none()).timeout(TIMEOUT);
    if state.config.allow_private_webhooks {
        return builder.build().ok();
    }
    let host = url.host_str()?;
    let address = public_address(host).await?;
    builder.resolve(host, address).build().ok()
}

/// Returns whether the far end accepted it, and whether trying again could help.
async fn post_once(state: &AppState, url: &Url, notice: &Notice) -> (bool, bool) {
    let Some(client) = client_for(state, url).await else {
        return (false, false);
    };
    let room = (!notice.hangout_id.is_empty()).then_some(notice.hangout_id.as_str());
    let link = match room {
        Some(id) => format!("{}/h/{id}", state.config.public_url),
        None => format!("{}/", state.config.public_url),
    };
    let request = client.post(url.clone());
    // ntfy shows the raw body as the phone notification, so it gets plain text.
    let request = if url.host_str() == Some(NTFY_HOST) {
        request
            .header("Title", "Agents Hangout")
            .header("Click", link.as_str())
            .body(notice.text.clone())
    } else {
        // `text` is what Slack and Teams read, and `content` is what Discord reads.
        request.json(&json!({
            "text": format!("{} {link}", notice.text),
            "content": format!("{} {link}", notice.text),
            "event": notice.event,
            "hangout_id": room,
            "link": link,
        }))
    };
    match request.send().await {
        Ok(response) if response.status().is_success() => (true, false),
        Ok(response) => (false, response.status().is_server_error()),
        Err(_) => (false, true),
    }
}

async fn deliver(state: &AppState, url: &str, notice: &Notice) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    for delay in RETRY_DELAYS {
        tokio::time::sleep(Duration::from_secs(delay)).await;
        let (accepted, retry) = post_once(state, &url, notice).await;
        if accepted {
            return true;
        }
        if !retry {
            break;
        }
    }
    // Notification URLs often contain secrets, so the log names only the event.
    tracing::warn!(event = notice.event, "notification was not delivered");
    false
}

/// Sends in the background, so a slow receiver never holds up whoever triggered this.
pub fn send(state: &AppState, owner_ids: &[String], notice: &Notice) {
    for owner_id in owner_ids {
        let Ok(Some(url)) = state.db.notify_url(owner_id) else {
            continue;
        };
        let state = state.clone();
        let notice = notice.clone();
        tokio::spawn(async move { deliver(&state, &url, &notice).await });
    }
}

/// Sends once and waits, so the page can say whether the URL works.
pub async fn send_test(state: &AppState, url: &str, hangout_id: &str) -> bool {
    let notice = Notice {
        event: "test",
        hangout_id: hangout_id.to_string(),
        text: "Agents Hangout can reach you here.".to_string(),
    };
    deliver(state, url, &notice).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(address: &str) -> bool {
        is_public(address.parse().unwrap())
    }

    #[test]
    fn internal_addresses_are_refused() {
        for address in [
            "169.254.169.254",
            "168.63.129.16",
            "127.0.0.1",
            "10.0.0.4",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:169.254.169.254",
            "::ffff:168.63.129.16",
        ] {
            assert!(!public(address), "{address}");
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for address in ["1.1.1.1", "13.107.42.14", "2606:4700:4700::1111"] {
            assert!(public(address), "{address}");
        }
    }
}
