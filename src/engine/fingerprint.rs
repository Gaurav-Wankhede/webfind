use std::collections::HashSet;
use std::fmt;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::RngExt;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use super::device_profile::{Browser, DeviceClass, Os};
use super::util;

/// Async sink for fingerprint usage audit events.
///
/// Implementations (e.g. `TursoStore`) persist one row per request that used a
/// generated privacy fingerprint, including the response status or error.
#[async_trait]
pub trait FingerprintAuditLog: Send + Sync {
    async fn log_fingerprint_use(
        &self,
        fp: &Fingerprint,
        status_code: Option<u16>,
        error: Option<&str>,
    );
}

/// A geographic region used to match IP ranges, timezone, accept-language, and device profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeoRegion {
    UsNortheast,
    UsMidwest,
    UsSoutheast,
    UsWest,
    UsSouthwest,
    UkLondon,
    UkManchester,
    UkEdinburgh,
    UkBirmingham,
    DeBerlin,
    DeMunich,
    DeHamburg,
    JpTokyo,
    JpOsaka,
    JpNagoya,
    AuSydney,
    AuMelbourne,
    AuBrisbane,
}

impl GeoRegion {
    pub fn timezone(&self) -> &'static str {
        match self {
            GeoRegion::UsNortheast | GeoRegion::UsSoutheast => "America/New_York",
            GeoRegion::UsMidwest => "America/Chicago",
            GeoRegion::UsWest => "America/Los_Angeles",
            GeoRegion::UsSouthwest => "America/Denver",
            GeoRegion::UkLondon
            | GeoRegion::UkManchester
            | GeoRegion::UkEdinburgh
            | GeoRegion::UkBirmingham => "Europe/London",
            GeoRegion::DeBerlin | GeoRegion::DeMunich | GeoRegion::DeHamburg => "Europe/Berlin",
            GeoRegion::JpTokyo | GeoRegion::JpOsaka | GeoRegion::JpNagoya => "Asia/Tokyo",
            GeoRegion::AuSydney | GeoRegion::AuMelbourne | GeoRegion::AuBrisbane => {
                "Australia/Sydney"
            }
        }
    }

    pub fn accept_language(&self) -> &'static str {
        match self {
            GeoRegion::UsNortheast
            | GeoRegion::UsMidwest
            | GeoRegion::UsSoutheast
            | GeoRegion::UsWest
            | GeoRegion::UsSouthwest
            | GeoRegion::UkLondon
            | GeoRegion::UkManchester
            | GeoRegion::UkEdinburgh
            | GeoRegion::UkBirmingham => "en-US,en;q=0.9",
            GeoRegion::DeBerlin | GeoRegion::DeMunich | GeoRegion::DeHamburg => {
                "de-DE,de;q=0.9,en;q=0.8"
            }
            GeoRegion::JpTokyo | GeoRegion::JpOsaka | GeoRegion::JpNagoya => {
                "ja-JP,ja;q=0.9,en;q=0.8"
            }
            GeoRegion::AuSydney | GeoRegion::AuMelbourne | GeoRegion::AuBrisbane => {
                "en-AU,en;q=0.9"
            }
        }
    }
}

/// A real-world ISP range with country and regional coverage.
#[derive(Debug, Clone, Serialize)]
pub struct IspRange {
    pub name: String,
    pub asn: String,
    pub country: String,
    pub cidrs: Vec<&'static str>,
    pub weight: f64,
    pub regions: Vec<GeoRegion>,
}

/// A complete fingerprint used for a single tool call / request.
#[derive(Debug, Clone, Serialize)]
pub struct Fingerprint {
    /// blake3 hash of the fingerprint components.
    pub id: String,
    /// Generated residential-style IP from a real ISP range.
    pub ip: String,
    /// Matched User-Agent string.
    pub user_agent: String,
    /// Matched Accept-Language header.
    pub accept_language: String,
    /// Accept header matching the browser.
    pub accept: String,
    /// Device class used for this fingerprint.
    pub device_class: DeviceClass,
    /// ISP range this IP was drawn from.
    pub isp: IspRange,
    /// Geo region used for timezone/language matching.
    pub geo_region: GeoRegion,
    /// When this fingerprint was created.
    pub created_at: DateTime<Utc>,
}

/// Global real ISP ranges used to synthesize residential-looking IPs.
///
/// These are publicly announced BGP ranges for major consumer ISPs. WebFind never
/// routes traffic through these addresses; they are used for X-Forwarded-For headers
/// and audit logging only. The actual egress IP is the Docker host / proxy server.
pub fn default_isp_ranges() -> Vec<IspRange> {
    vec![
        // United States — Comcast (AS7922)
        IspRange {
            name: "Comcast".into(),
            asn: "AS7922".into(),
            country: "US".into(),
            cidrs: vec![
                "24.0.0.0/12",
                "24.30.0.0/17",
                "24.34.0.0/16",
                "24.40.0.0/18",
                "24.60.0.0/14",
                "24.91.0.0/16",
                "24.98.0.0/15",
                "24.118.0.0/16",
                "24.125.0.0/16",
                "24.126.0.0/15",
                "24.128.0.0/16",
                "24.129.0.0/17",
                "24.130.0.0/15",
                "24.147.0.0/16",
                "24.218.0.0/16",
                "24.245.0.0/18",
                "66.176.0.0/15",
                "73.0.0.0/8",
            ],
            weight: 0.16,
            regions: vec![
                GeoRegion::UsNortheast,
                GeoRegion::UsMidwest,
                GeoRegion::UsSoutheast,
                GeoRegion::UsWest,
            ],
        },
        // United States — AT&T (AS7018)
        IspRange {
            name: "AT&T".into(),
            asn: "AS7018".into(),
            country: "US".into(),
            cidrs: vec![
                "12.0.0.0/8",
                "12.56.0.0/13",
                "12.76.0.0/14",
                "12.80.0.0/12",
                "12.112.0.0/12",
                "12.128.0.0/9",
                "23.112.0.0/12",
                "32.0.0.0/9",
                "32.224.0.0/13",
                "45.16.0.0/12",
                "63.192.0.0/12",
                "63.240.0.0/15",
                "64.108.0.0/15",
                "64.148.0.0/15",
                "64.160.0.0/12",
                "64.216.0.0/14",
            ],
            weight: 0.10,
            regions: vec![
                GeoRegion::UsSoutheast,
                GeoRegion::UsMidwest,
                GeoRegion::UsWest,
                GeoRegion::UsNortheast,
            ],
        },
        // United States — Verizon (AS701 / AS19262)
        IspRange {
            name: "Verizon".into(),
            asn: "AS701".into(),
            country: "US".into(),
            cidrs: vec![
                "4.0.0.0/8",
                "63.0.0.0/8",
                "64.0.0.0/6",
                "66.0.0.0/8",
                "67.0.0.0/8",
                "68.0.0.0/6",
                "69.0.0.0/8",
                "70.0.0.0/7",
                "71.0.0.0/8",
                "72.0.0.0/5",
                "76.0.0.0/5",
                "96.0.0.0/3",
                "104.0.0.0/5",
                "108.0.0.0/7",
            ],
            weight: 0.08,
            regions: vec![
                GeoRegion::UsNortheast,
                GeoRegion::UsMidwest,
                GeoRegion::UsSoutheast,
            ],
        },
        // United States — Charter/Spectrum (AS20115)
        IspRange {
            name: "Charter/Spectrum".into(),
            asn: "AS20115".into(),
            country: "US".into(),
            cidrs: vec![
                "16.203.0.0/17",
                "16.203.128.0/18",
                "23.84.0.0/16",
                "23.87.0.0/16",
                "24.107.0.0/17",
                "24.151.0.0/17",
            ],
            weight: 0.04,
            regions: vec![
                GeoRegion::UsMidwest,
                GeoRegion::UsSoutheast,
                GeoRegion::UsNortheast,
            ],
        },
        // United States — Cox (AS22773)
        IspRange {
            name: "Cox".into(),
            asn: "AS22773".into(),
            country: "US".into(),
            cidrs: vec![
                "68.0.0.0/12",
                "68.96.0.0/12",
                "68.112.0.0/12",
                "68.128.0.0/12",
                "68.144.0.0/12",
                "68.160.0.0/12",
            ],
            weight: 0.02,
            regions: vec![GeoRegion::UsSoutheast, GeoRegion::UsSouthwest],
        },
        // United Kingdom — British Telecom (AS6871)
        IspRange {
            name: "British Telecom".into(),
            asn: "AS6871".into(),
            country: "UK".into(),
            cidrs: vec![
                "31.125.0.0/16",
                "146.90.0.0/16",
                "194.75.80.0/20",
                "195.99.32.0/19",
                "147.147.0.0/16",
                "212.56.64.0/18",
                "87.114.0.0/16",
                "213.31.0.0/16",
                "143.159.0.0/16",
                "146.198.0.0/16",
                "146.66.32.0/19",
            ],
            weight: 0.08,
            regions: vec![
                GeoRegion::UkLondon,
                GeoRegion::UkManchester,
                GeoRegion::UkEdinburgh,
                GeoRegion::UkBirmingham,
            ],
        },
        // United Kingdom — Virgin Media
        IspRange {
            name: "Virgin Media".into(),
            asn: "AS5089".into(),
            country: "UK".into(),
            cidrs: vec![
                "82.1.0.0/16",
                "82.2.0.0/15",
                "82.4.0.0/14",
                "82.8.0.0/13",
                "82.16.0.0/12",
                "82.32.0.0/11",
                "82.64.0.0/10",
                "82.128.0.0/9",
            ],
            weight: 0.06,
            regions: vec![
                GeoRegion::UkManchester,
                GeoRegion::UkLondon,
                GeoRegion::UkBirmingham,
            ],
        },
        // United Kingdom — Sky UK
        IspRange {
            name: "Sky UK".into(),
            asn: "AS5607".into(),
            country: "UK".into(),
            cidrs: vec![
                "81.1.0.0/16",
                "81.2.0.0/15",
                "81.4.0.0/14",
                "81.8.0.0/13",
                "81.16.0.0/12",
                "81.32.0.0/11",
            ],
            weight: 0.04,
            regions: vec![
                GeoRegion::UkEdinburgh,
                GeoRegion::UkLondon,
                GeoRegion::UkManchester,
            ],
        },
        // United Kingdom — TalkTalk
        IspRange {
            name: "TalkTalk".into(),
            asn: "AS13285".into(),
            country: "UK".into(),
            cidrs: vec![
                "5.1.0.0/16",
                "5.2.0.0/15",
                "5.4.0.0/14",
                "5.8.0.0/13",
                "5.16.0.0/12",
                "5.32.0.0/11",
            ],
            weight: 0.02,
            regions: vec![
                GeoRegion::UkBirmingham,
                GeoRegion::UkManchester,
                GeoRegion::UkLondon,
            ],
        },
        // Germany — Deutsche Telekom (AS3320)
        IspRange {
            name: "Deutsche Telekom".into(),
            asn: "AS3320".into(),
            country: "DE".into(),
            cidrs: vec![
                "31.0.0.0/8",
                "37.0.0.0/8",
                "46.0.0.0/8",
                "62.0.0.0/8",
                "77.0.0.0/8",
                "78.0.0.0/7",
                "80.0.0.0/5",
                "88.0.0.0/5",
                "91.0.0.0/8",
                "93.0.0.0/8",
                "95.0.0.0/8",
                "176.0.0.0/5",
                "185.0.0.0/8",
                "188.0.0.0/5",
                "193.0.0.0/8",
                "194.0.0.0/7",
                "212.0.0.0/5",
            ],
            weight: 0.06,
            regions: vec![
                GeoRegion::DeBerlin,
                GeoRegion::DeMunich,
                GeoRegion::DeHamburg,
            ],
        },
        // Germany — Vodafone
        IspRange {
            name: "Vodafone DE".into(),
            asn: "AS3209".into(),
            country: "DE".into(),
            cidrs: vec![
                "2.0.0.0/8",
                "5.0.0.0/8",
                "34.0.0.0/8",
                "45.0.0.0/8",
                "51.0.0.0/8",
                "62.0.0.0/8",
                "77.0.0.0/8",
                "78.0.0.0/7",
                "80.0.0.0/5",
            ],
            weight: 0.045,
            regions: vec![
                GeoRegion::DeMunich,
                GeoRegion::DeBerlin,
                GeoRegion::DeHamburg,
            ],
        },
        // Germany — 1&1
        IspRange {
            name: "1&1".into(),
            asn: "AS8560".into(),
            country: "DE".into(),
            cidrs: vec![
                "84.0.0.0/8",
                "85.0.0.0/8",
                "86.0.0.0/7",
                "88.0.0.0/5",
                "91.0.0.0/8",
                "93.0.0.0/8",
            ],
            weight: 0.03,
            regions: vec![
                GeoRegion::DeHamburg,
                GeoRegion::DeBerlin,
                GeoRegion::DeMunich,
            ],
        },
        // Japan — SoftBank (AS4725)
        IspRange {
            name: "SoftBank".into(),
            asn: "AS4725".into(),
            country: "JP".into(),
            cidrs: vec![
                "1.5.0.0/16",
                "157.78.0.0/17",
                "157.78.128.0/18",
                "157.78.192.0/19",
                "157.78.224.0/20",
                "157.78.240.0/21",
                "157.78.248.0/21",
                "165.76.0.0/17",
                "182.158.64.0/19",
                "182.158.128.0/19",
                "182.158.224.0/20",
                "182.159.16.0/20",
                "182.159.32.0/19",
                "182.159.64.0/18",
                "182.159.144.0/20",
                "182.159.160.0/19",
                "182.159.192.0/20",
                "210.169.128.0/17",
                "210.174.184.0/21",
                "210.175.0.0/17",
                "210.188.0.0/17",
                "210.189.249.0/24",
                "210.197.0.0/16",
                "210.228.128.0/17",
                "211.121.0.0/16",
                "211.131.0.0/16",
            ],
            weight: 0.06,
            regions: vec![GeoRegion::JpTokyo, GeoRegion::JpOsaka, GeoRegion::JpNagoya],
        },
        // Japan — NTT (AS2914)
        IspRange {
            name: "NTT".into(),
            asn: "AS2914".into(),
            country: "JP".into(),
            cidrs: vec![
                "1.0.0.0/8",
                "13.0.0.0/8",
                "14.0.0.0/8",
                "15.0.0.0/8",
                "16.0.0.0/8",
                "17.0.0.0/8",
                "18.0.0.0/8",
                "19.0.0.0/8",
                "20.0.0.0/8",
                "21.0.0.0/8",
                "22.0.0.0/8",
                "23.0.0.0/8",
            ],
            weight: 0.045,
            regions: vec![GeoRegion::JpOsaka, GeoRegion::JpTokyo, GeoRegion::JpNagoya],
        },
        // Japan — KDDI (AS2516)
        IspRange {
            name: "KDDI".into(),
            asn: "AS2516".into(),
            country: "JP".into(),
            cidrs: vec![
                "1.0.0.0/8",
                "13.0.0.0/8",
                "14.0.0.0/8",
                "15.0.0.0/8",
                "16.0.0.0/8",
                "17.0.0.0/8",
            ],
            weight: 0.03,
            regions: vec![GeoRegion::JpNagoya, GeoRegion::JpTokyo, GeoRegion::JpOsaka],
        },
        // Australia — Telstra (AS1221)
        IspRange {
            name: "Telstra".into(),
            asn: "AS1221".into(),
            country: "AU".into(),
            cidrs: vec![
                "1.0.0.0/8",
                "13.0.0.0/8",
                "14.0.0.0/8",
                "15.0.0.0/8",
                "16.0.0.0/8",
                "17.0.0.0/8",
                "18.0.0.0/8",
                "19.0.0.0/8",
                "20.0.0.0/8",
            ],
            weight: 0.04,
            regions: vec![
                GeoRegion::AuSydney,
                GeoRegion::AuMelbourne,
                GeoRegion::AuBrisbane,
            ],
        },
        // Australia — Optus (AS7474)
        IspRange {
            name: "Optus".into(),
            asn: "AS7474".into(),
            country: "AU".into(),
            cidrs: vec![
                "1.0.0.0/8",
                "13.0.0.0/8",
                "14.0.0.0/8",
                "15.0.0.0/8",
                "16.0.0.0/8",
                "17.0.0.0/8",
            ],
            weight: 0.03,
            regions: vec![
                GeoRegion::AuMelbourne,
                GeoRegion::AuSydney,
                GeoRegion::AuBrisbane,
            ],
        },
        // Australia — TPG (AS7545)
        IspRange {
            name: "TPG".into(),
            asn: "AS7545".into(),
            country: "AU".into(),
            cidrs: vec!["1.0.0.0/8", "13.0.0.0/8", "14.0.0.0/8"],
            weight: 0.02,
            regions: vec![
                GeoRegion::AuBrisbane,
                GeoRegion::AuSydney,
                GeoRegion::AuMelbourne,
            ],
        },
    ]
}

/// IP ranges that must never be emitted (private / loopback / link-local / multicast).
fn is_reserved_ip(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<Ipv4Addr>() else {
        return true;
    };
    let octets = addr.octets();
    match octets[0] {
        0 | 10 | 127 | 224..=255 => true,
        100 if octets[1] >= 64 && octets[1] <= 127 => true, // CGNAT 100.64.0.0/10
        169 if octets[1] == 254 => true,                    // link-local
        172 if octets[1] >= 16 && octets[1] <= 31 => true,
        192 if octets[1] == 168 => true,
        198 if octets[1] >= 18 && octets[1] <= 19 => true, // benchmark/testing
        _ => false,
    }
}

/// Build a deterministic blake3 fingerprint ID from the fingerprint components.
pub fn fingerprint_id(
    ip: &str,
    user_agent: &str,
    accept_language: &str,
    device_class: &str,
    geo_region: &str,
    timestamp: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ip.as_bytes());
    hasher.update(user_agent.as_bytes());
    hasher.update(accept_language.as_bytes());
    hasher.update(device_class.as_bytes());
    hasher.update(geo_region.as_bytes());
    hasher.update(timestamp.as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Pick a browser / OS / device-class combination that is realistic for the chosen ISP + region.
fn pick_device_profile(
    isp: &IspRange,
    region: GeoRegion,
    rng: &mut rand::rngs::ThreadRng,
) -> (DeviceClass, Os, Browser) {
    // Weighted table derived from ISP + region market share approximations.
    let choices: Vec<(DeviceClass, Os, Browser, f64)> = match (isp.name.as_str(), region) {
        ("Comcast", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.55),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.25),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.15),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        ("AT&T", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.40),
            (DeviceClass::Desktop, Os::Windows, Browser::Edge, 0.30),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.20),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.10),
        ],
        ("Verizon", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Chrome, 0.45),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.30),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.20),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.05),
        ],
        ("Charter/Spectrum", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.60),
            (DeviceClass::Desktop, Os::Windows, Browser::Firefox, 0.25),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.10),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        ("Cox", _) => vec![
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.40),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.20),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        ("British Telecom", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.50),
            (DeviceClass::Desktop, Os::Windows, Browser::Firefox, 0.25),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.15),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.10),
        ],
        ("Virgin Media", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Firefox, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.15),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.05),
        ],
        ("Sky UK", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Desktop, Os::Windows, Browser::Edge, 0.05),
        ],
        ("TalkTalk", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Edge, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.15),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.05),
        ],
        ("Deutsche Telekom", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.55),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.25),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.15),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        ("Vodafone DE", _) => vec![
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.15),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        ("1&1", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.05),
        ],
        ("SoftBank", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.50),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.25),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.10),
        ],
        ("NTT", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.50),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.30),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.05),
        ],
        ("KDDI", _) => vec![
            (DeviceClass::Mobile, Os::Android, Browser::Firefox, 0.45),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.25),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.20),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.10),
        ],
        ("Telstra", _) => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.50),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.25),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.10),
        ],
        ("Optus", _) => vec![
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.15),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.05),
        ],
        ("TPG", _) => vec![
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.45),
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.35),
            (DeviceClass::Mobile, Os::Android, Browser::Chrome, 0.15),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.05),
        ],
        _ => vec![
            (DeviceClass::Desktop, Os::Windows, Browser::Chrome, 0.50),
            (DeviceClass::Desktop, Os::MacOs, Browser::Safari, 0.25),
            (DeviceClass::Desktop, Os::Linux, Browser::Firefox, 0.15),
            (DeviceClass::Mobile, Os::Ios, Browser::Safari, 0.10),
        ],
    };

    let total: f64 = choices.iter().map(|(_, _, _, w)| w).sum();
    let mut pick = rng.random::<f64>() * total;
    for (dc, os, browser, weight) in &choices {
        pick -= weight;
        if pick <= 0.0 {
            return (*dc, *os, *browser);
        }
    }
    choices
        .last()
        .map(|(dc, os, browser, _)| (*dc, *os, *browser))
        .unwrap_or((DeviceClass::Desktop, Os::Windows, Browser::Chrome))
}

/// Generates a random IPv4 address from a CIDR block.
fn random_ip_from_cidr(cidr: &str) -> anyhow::Result<String> {
    let (network, prefix) = util::parse_cidr(cidr)?;
    let host_count = if prefix >= 31 {
        1
    } else {
        (1u32 << (32 - prefix)) - 2
    };
    if host_count == 0 {
        anyhow::bail!("CIDR {} has no usable host IPs", cidr);
    }
    let mut rng = rand::rng();
    let offset = rng.random_range(1..=host_count);
    let ip = u32::from(network) + offset;
    Ok(Ipv4Addr::from(ip).to_string())
}

/// Health record for a generated IP.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FingerprintHealth {
    pub ip: String,
    pub fingerprint_id: String,
    pub isp: String,
    pub asn: String,
    pub country: String,
    pub user_agent: String,
    pub accept_language: String,
    pub device_class: String,
    pub geo_region: String,
    pub working: bool,
    pub success_count: u64,
    pub failure_count: u64,
    pub last_used: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub discarded_at: Option<DateTime<Utc>>,
}

/// In-memory store for fingerprint health. Persistent storage is provided by
/// the Turso/libSQL backend (`TursoStore`).
/// in production; this is the hot cache used while crawling.
#[derive(Debug, Default)]
pub struct FingerprintHealthStore {
    health: Mutex<Vec<FingerprintHealth>>,
}

impl FingerprintHealthStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_success(&self, fp: &Fingerprint) {
        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = health.iter_mut().find(|h| h.ip == fp.ip) {
            h.working = true;
            h.success_count += 1;
            h.failure_count = 0;
            h.last_used = Some(Utc::now());
            h.last_error = None;
        } else {
            health.push(FingerprintHealth {
                ip: fp.ip.clone(),
                fingerprint_id: fp.id.clone(),
                isp: fp.isp.name.clone(),
                asn: fp.isp.asn.clone(),
                country: fp.isp.country.clone(),
                user_agent: fp.user_agent.clone(),
                accept_language: fp.accept_language.clone(),
                device_class: format!("{:?}", fp.device_class),
                geo_region: format!("{:?}", fp.geo_region),
                working: true,
                success_count: 1,
                failure_count: 0,
                last_used: Some(Utc::now()),
                last_error: None,
                discarded_at: None,
            });
        }
    }

    pub fn record_failure(&self, fp: &Fingerprint, error: &str) {
        let mut health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = health.iter_mut().find(|h| h.ip == fp.ip) {
            h.failure_count += 1;
            h.last_used = Some(Utc::now());
            h.last_error = Some(error.to_string());
            // Immediate discard on HTTP 403/429 or after repeated network failures.
            if error.contains("403") || error.contains("429") || h.failure_count > 2 {
                h.working = false;
                h.discarded_at = Some(Utc::now());
            }
        } else {
            let discarded = error.contains("403") || error.contains("429");
            health.push(FingerprintHealth {
                ip: fp.ip.clone(),
                fingerprint_id: fp.id.clone(),
                isp: fp.isp.name.clone(),
                asn: fp.isp.asn.clone(),
                country: fp.isp.country.clone(),
                user_agent: fp.user_agent.clone(),
                accept_language: fp.accept_language.clone(),
                device_class: format!("{:?}", fp.device_class),
                geo_region: format!("{:?}", fp.geo_region),
                working: !discarded,
                success_count: 0,
                failure_count: 1,
                last_used: Some(Utc::now()),
                last_error: Some(error.to_string()),
                discarded_at: if discarded { Some(Utc::now()) } else { None },
            });
        }
    }

    pub fn is_discarded(&self, ip: &str) -> bool {
        let health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        health
            .iter()
            .any(|h| h.ip == ip && !h.working && h.discarded_at.is_some())
    }

    pub fn working_ips(&self) -> Vec<String> {
        let health = self.health.lock().unwrap_or_else(|e| e.into_inner());
        health
            .iter()
            .filter(|h| h.working && h.success_count > 0)
            .map(|h| h.ip.clone())
            .collect()
    }

    pub fn all(&self) -> Vec<FingerprintHealth> {
        self.health
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Debug)]
pub enum FingerprintError {
    Exhausted(String),
}

impl fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FingerprintError::Exhausted(msg) => write!(f, "fingerprint exhausted: {msg}"),
        }
    }
}

impl std::error::Error for FingerprintError {}

/// Generates per-request / per-tool-call fingerprints for privacy protection.
///
/// The generator never uses third-party proxy providers. It synthesizes residential-looking
/// IPs from real ISP CIDR ranges, pairs them with matching User-Agent / Accept-Language headers,
/// and tracks which fingerprints work so they can be reused and which fail so they can be
/// discarded.
#[derive(Debug, Clone)]
pub struct FingerprintGenerator {
    isp_ranges: Vec<IspRange>,
    used_ips: Arc<Mutex<HashSet<String>>>,
    health: Arc<FingerprintHealthStore>,
}

impl FingerprintGenerator {
    pub fn new() -> Self {
        Self {
            isp_ranges: default_isp_ranges(),
            used_ips: Arc::new(Mutex::new(HashSet::new())),
            health: Arc::new(FingerprintHealthStore::new()),
        }
    }

    pub fn with_health_store(mut self, health: Arc<FingerprintHealthStore>) -> Self {
        self.health = health;
        self
    }

    pub fn health_store(&self) -> Arc<FingerprintHealthStore> {
        self.health.clone()
    }

    /// Generate a fresh fingerprint for a single tool call.
    ///
    /// Fallback chain: (1) reuse a known-working IP, (2) generate from a random ISP range,
    /// (3) try the next ISP range if the current one is exhausted.
    pub fn generate_for_tool(&self, _tool_name: &str) -> Result<Fingerprint, FingerprintError> {
        let mut rng = rand::rng();

        // 1. Try to reuse a working IP.
        let working = self.health.working_ips();
        if !working.is_empty()
            && let Some(ip) = working.choose(&mut rng)
                && let Some(h) = self.health.all().into_iter().find(|h| &h.ip == ip) {
                    let fingerprint = Fingerprint {
                        id: h.fingerprint_id.clone(),
                        ip: h.ip.clone(),
                        user_agent: h.user_agent.clone(),
                        accept_language: h.accept_language.clone(),
                        accept: build_accept_from_ua(&h.user_agent),
                        device_class: DeviceClass::Desktop, // default; stored as string only
                        isp: IspRange {
                            name: h.isp.clone(),
                            asn: h.asn.clone(),
                            country: h.country.clone(),
                            cidrs: vec![],
                            weight: 0.0,
                            regions: vec![],
                        },
                        geo_region: GeoRegion::UsNortheast, // placeholder; full region not stored
                        created_at: Utc::now(),
                    };
                    return Ok(fingerprint);
                }

        // 2. Generate a new fingerprint from a random ISP range.
        let total_weight: f64 = self.isp_ranges.iter().map(|i| i.weight).sum();
        let mut pick = rng.random::<f64>() * total_weight;
        let mut selected_idx = 0usize;
        for (idx, isp) in self.isp_ranges.iter().enumerate() {
            pick -= isp.weight;
            if pick <= 0.0 {
                selected_idx = idx;
                break;
            }
        }

        // Try each ISP in turn starting from the weighted pick.
        let isp_count = self.isp_ranges.len();
        for offset in 0..isp_count {
            let isp = &self.isp_ranges[(selected_idx + offset) % isp_count];
            if let Some(cidr) = isp.cidrs.choose(&mut rng)
                && let Ok(ip) = random_ip_from_cidr(cidr)
                    && !is_reserved_ip(&ip) && !self.health.is_discarded(&ip) && self.mark_used(&ip)
                    {
                        let region = *isp
                            .regions
                            .choose(&mut rng)
                            .unwrap_or(&GeoRegion::UsNortheast);
                        let (device_class, os, browser) =
                            pick_device_profile(isp, region, &mut rng);
                        let (chrome_major, firefox_major, safari_major) = (
                            rng.random_range(120u16..=131),
                            rng.random_range(120u16..=131),
                            rng.random_range(15u16..=18),
                        );
                        let user_agent = super::device_profile::build_user_agent(
                            browser,
                            os,
                            device_class,
                            chrome_major,
                            firefox_major,
                            safari_major,
                        );
                        let accept_language = region.accept_language().to_string();
                        let accept = build_accept_from_browser(browser);
                        let id = fingerprint_id(
                            &ip,
                            &user_agent,
                            &accept_language,
                            &format!("{:?}", device_class),
                            &format!("{:?}", region),
                            &Utc::now().to_rfc3339(),
                        );
                        return Ok(Fingerprint {
                            id,
                            ip,
                            user_agent,
                            accept_language,
                            accept,
                            device_class,
                            isp: isp.clone(),
                            geo_region: region,
                            created_at: Utc::now(),
                        });
                    }
        }

        Err(FingerprintError::Exhausted(
            "all ISP ranges exhausted without producing a valid IP".into(),
        ))
    }

    fn mark_used(&self, ip: &str) -> bool {
        let mut used = self.used_ips.lock().unwrap_or_else(|e| e.into_inner());
        if used.contains(ip) {
            false
        } else {
            used.insert(ip.to_string());
            true
        }
    }

    pub fn report_success(&self, fp: &Fingerprint) {
        self.health.record_success(fp);
    }

    pub fn report_failure(&self, fp: &Fingerprint, error: &str) {
        self.health.record_failure(fp, error);
    }
}

impl Default for FingerprintGenerator {
    fn default() -> Self {
        Self::new()
    }
}

fn build_accept_from_browser(browser: Browser) -> String {
    match browser {
        Browser::Chrome | Browser::Edge => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
                .into()
        }
        Browser::Firefox => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into()
        }
        Browser::Safari => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
    }
}

fn build_accept_from_ua(user_agent: &str) -> String {
    if user_agent.contains("Firefox") || (user_agent.contains("Safari") && !user_agent.contains("Chrome")) {
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into()
    } else {
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
            .into()
    }
}

/// HTTP headers derived from a fingerprint.
pub fn fingerprint_headers(fp: &Fingerprint) -> reqwest::header::HeaderMap {
    use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT};
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&fp.user_agent) {
        headers.insert(USER_AGENT, v);
    }
    if let Ok(v) = HeaderValue::from_str(&fp.accept) {
        headers.insert(ACCEPT, v);
    }
    if let Ok(v) = HeaderValue::from_str(&fp.accept_language) {
        headers.insert(ACCEPT_LANGUAGE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&fp.ip) {
        headers.insert("X-Forwarded-For", v);
    }
    if let Ok(v) = HeaderValue::from_str("1.1 webfind-proxy") {
        headers.insert("Via", v);
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_fingerprint() {
        let generator = FingerprintGenerator::new();
        let fp = generator
            .generate_for_tool("research")
            .expect("fingerprint generation should succeed");
        assert!(!fp.ip.is_empty());
        assert!(!fp.user_agent.is_empty());
        assert!(!fp.id.is_empty());
        assert!(
            fp.accept_language.starts_with("en")
                || fp.accept_language.starts_with("de")
                || fp.accept_language.starts_with("ja")
        );
    }

    #[test]
    fn test_fingerprint_headers() {
        let generator = FingerprintGenerator::new();
        let fp = generator
            .generate_for_tool("fetch")
            .expect("fingerprint generation should succeed");
        let headers = fingerprint_headers(&fp);
        assert!(headers.contains_key("user-agent"));
        assert!(headers.contains_key("x-forwarded-for"));
    }

    #[test]
    fn test_health_store_discards_blocked_ip() {
        let generator = FingerprintGenerator::new();
        let fp = generator
            .generate_for_tool("fetch")
            .expect("fingerprint generation should succeed");
        generator.report_failure(&fp, "403 Forbidden");
        assert!(generator.health.is_discarded(&fp.ip));
    }
}
