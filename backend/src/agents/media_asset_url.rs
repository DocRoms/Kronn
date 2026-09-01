//! Validation of the download URLs a provider hands back.
//!
//! A completed generation answers with URLs we did not write. Following one
//! blindly turns the backend into a fetcher for whatever the payload names —
//! and the download carries the provider credential, so a hostile URL would
//! also exfiltrate it. Every asset URL therefore goes through here first, and
//! the credential is attached only to hosts the codec vouches for.

use anyhow::{bail, Result};
use reqwest::Url;

/// Which hosts a codec is willing to fetch an asset from.
///
/// Suffixes starting with a dot match the domain and its subdomains
/// (`.nvidia.com` matches `nvidia.com` and `ai.api.nvidia.com`, never
/// `evil-nvidia.com`); any other entry must match the host exactly.
#[derive(Debug, Clone, Copy)]
pub struct AssetHostPolicy {
    /// Hosts the provider credential may be sent to, because their content
    /// endpoint requires it.
    pub credentialed: &'static [&'static str],
    /// Hosts allowed for an anonymous download — pre-signed storage, where
    /// the URL is the authorisation and a Bearer would only leak.
    pub anonymous: &'static [&'static str],
}

impl AssetHostPolicy {
    pub const fn credentialed_only(hosts: &'static [&'static str]) -> Self {
        Self {
            credentialed: hosts,
            anonymous: &[],
        }
    }
}

/// An asset URL cleared for download, and whether it may carry the credential.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedAssetUrl {
    pub url: String,
    pub send_credential: bool,
}

/// Clears one provider-supplied URL against the codec policy and the base the
/// operator configured.
///
/// The configured base is trusted on its own terms: a self-hosted LiteLLM is
/// legitimately `http://127.0.0.1:4000`, and refusing it would break a working
/// deployment. Any OTHER host must be an https name — no plaintext, no IP
/// literal, no port games — and be vouched for by the policy.
pub fn validate_asset_url(
    candidate: &str,
    base: &str,
    policy: &AssetHostPolicy,
) -> Result<ValidatedAssetUrl> {
    let url = Url::parse(candidate.trim())
        .map_err(|e| anyhow::anyhow!("provider returned an unusable asset url: {e}"))?;

    let host = match url.host_str() {
        Some(host) if !host.is_empty() => host.to_ascii_lowercase(),
        _ => bail!("provider asset url carries no host"),
    };

    // `https://openrouter.ai@evil.example/x` parses with host `evil.example`,
    // so host checking already covers the trick; userinfo is refused anyway
    // because a legitimate asset URL never carries credentials inline.
    if !url.username().is_empty() || url.password().is_some() {
        bail!("provider asset url embeds credentials, refused");
    }

    // Same ORIGIN as the configured endpoint — scheme, host AND port. Port
    // matters: on a self-hosted box the gateway's neighbours (an Ollama on
    // 11434, an admin panel) share the host, and matching on host alone would
    // hand them the credential.
    if origin(&url) == Url::parse(base.trim()).ok().and_then(|b| origin(&b)) {
        return Ok(ValidatedAssetUrl {
            url: url.to_string(),
            send_credential: true,
        });
    }

    if url.scheme() != "https" {
        bail!(
            "provider asset url uses {}, only https is followed off the configured host",
            url.scheme()
        );
    }
    if url.port().is_some_and(|port| port != 443) {
        bail!("provider asset url points at a non-standard port, refused");
    }
    // An IP literal cannot be vouched for by name, and is the shape an SSRF
    // attempt takes (169.254.169.254, 127.0.0.1, 10.x). IPv6 hosts serialise
    // bracketed, hence the trim.
    if host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok()
    {
        bail!("provider asset url points at a raw IP address, refused");
    }

    if matches(&host, policy.credentialed) {
        return Ok(ValidatedAssetUrl {
            url: url.to_string(),
            send_credential: true,
        });
    }
    if matches(&host, policy.anonymous) {
        return Ok(ValidatedAssetUrl {
            url: url.to_string(),
            send_credential: false,
        });
    }

    // Named on purpose: a provider that starts serving assets from a new host
    // should surface as a readable refusal, not a silent download.
    bail!("provider asset url host {host} is not an allowed asset host")
}

/// Scheme + host + effective port, so a different port is a different origin.
fn origin(url: &Url) -> Option<(String, String, u16)> {
    Some((
        url.scheme().to_string(),
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default()?,
    ))
}

fn matches(host: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        match entry.strip_prefix('.') {
            Some(domain) => host == domain || host.ends_with(&entry),
            None => host == entry,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://openrouter.ai/api/v1";
    const POLICY: AssetHostPolicy = AssetHostPolicy::credentialed_only(&["openrouter.ai"]);

    #[test]
    fn the_real_openrouter_content_url_is_cleared_with_the_credential() {
        let cleared = validate_asset_url(
            "https://openrouter.ai/api/v1/videos/Nu908SAyQg81UNYgC4Xh/content?index=0",
            BASE,
            &POLICY,
        )
        .expect("the provider's own content url must stay downloadable");
        assert!(cleared.send_credential);
    }

    #[test]
    fn hostile_urls_are_refused_before_any_request_is_made() {
        // Each of these is a payload a provider (or anything able to answer as
        // one) could return; none may be fetched, and none may see the key.
        for hostile in [
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "https://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:11434/api/tags",
            "https://attacker.example/collect",
            "https://openrouter.ai.attacker.example/api/v1/videos/x/content",
            "https://evil-openrouter.ai/content",
            "file:///etc/passwd",
            "https://openrouter.ai:8443/api/v1/videos/x/content",
            "https://user:secret@openrouter.ai/api/v1/videos/x/content",
        ] {
            let refused = validate_asset_url(hostile, BASE, &POLICY);
            assert!(refused.is_err(), "{hostile} must be refused");
        }
    }

    #[test]
    fn a_subdomain_is_cleared_only_when_the_policy_says_so() {
        let suffixed = AssetHostPolicy::credentialed_only(&[".nvidia.com"]);
        assert!(validate_asset_url("https://ai.api.nvidia.com/v1/x.mp4", "", &suffixed).is_ok());
        assert!(validate_asset_url("https://nvidia.com/v1/x.mp4", "", &suffixed).is_ok());
        assert!(validate_asset_url("https://notnvidia.com/v1/x.mp4", "", &suffixed).is_err());
        // Exact entries do not gain subdomains implicitly.
        assert!(validate_asset_url("https://cdn.openrouter.ai/x.mp4", BASE, &POLICY).is_err());
    }

    #[test]
    fn a_pre_signed_storage_host_is_fetched_without_the_credential() {
        let policy = AssetHostPolicy {
            credentialed: &["openrouter.ai"],
            anonymous: &[".blob.core.example"],
        };
        let cleared =
            validate_asset_url("https://a.blob.core.example/asset.mp4?sig=x", BASE, &policy)
                .expect("pre-signed storage must remain downloadable");
        assert!(
            !cleared.send_credential,
            "a pre-signed URL is its own authorisation; sending the key there leaks it"
        );
    }

    #[test]
    fn a_self_hosted_base_stays_usable_on_its_own_host() {
        let base = "http://127.0.0.1:4000";
        let cleared = validate_asset_url("http://127.0.0.1:4000/v1/files/a.mp4", base, &POLICY)
            .expect("a self-hosted gateway must be able to serve its own assets");
        assert!(cleared.send_credential);
        // ...but only that host, not any plaintext neighbour.
        assert!(validate_asset_url("http://127.0.0.1:11434/x.mp4", base, &POLICY).is_err());
    }
}
