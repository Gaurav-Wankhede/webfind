use std::collections::HashMap;
use std::sync::Mutex;

use rand::RngExt;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

/// A device fingerprint profile that must stay consistent for a crawling session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    pub id: String,
    pub browser: Browser,
    pub os: Os,
    pub device_class: DeviceClass,
    pub user_agent: String,
    pub accept: String,
    pub accept_language: String,
    pub accept_encoding: String,
    pub sec_ch_ua: Option<String>,
    pub sec_ch_ua_mobile: Option<String>,
    pub sec_ch_ua_platform: Option<String>,
    pub viewport: Viewport,
    pub device_pixel_ratio: f32,
    pub timezone: String,
    pub hardware_concurrency: u8,
    pub device_memory: u8,
    pub dnt: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Browser {
    Chrome,
    Firefox,
    Safari,
    Edge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    Android,
    Ios,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceClass {
    Desktop,
    Mobile,
    Tablet,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

/// Timezones grouped by rough geographic region for geo-matching.
const TIMEZONES: &[&str] = &[
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Toronto",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Madrid",
    "Europe/Moscow",
    "Asia/Tokyo",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Australia/Sydney",
    "Pacific/Auckland",
];

impl DeviceProfile {
    /// Generate a realistic device fingerprint completely at random.
    /// No hardcoded presets — every field is synthesized from current browser ranges.
    pub fn random() -> Self {
        let mut rng = rand::rng();
        let os = *[Os::Windows, Os::MacOs, Os::Linux, Os::Android, Os::Ios]
            .choose(&mut rng)
            .expect("os array non-empty");
        let device_class = match os {
            Os::Android | Os::Ios => *[DeviceClass::Mobile, DeviceClass::Tablet]
                .choose(&mut rng)
                .expect("mobile/tablet array non-empty"),
            _ => DeviceClass::Desktop,
        };
        let browser = *[
            Browser::Chrome,
            Browser::Firefox,
            Browser::Safari,
            Browser::Edge,
        ]
        .choose(&mut rng)
        .expect("browser array non-empty");

        let (chrome_major, firefox_major, safari_major) = (
            rng.random_range(120u16..=131),
            rng.random_range(120u16..=131),
            rng.random_range(15u16..=18),
        );

        let user_agent = build_user_agent(
            browser,
            os,
            device_class,
            chrome_major,
            firefox_major,
            safari_major,
        );
        let accept = build_accept(browser);
        let accept_language = build_accept_language();
        let accept_encoding = build_accept_encoding(browser);
        let sec_ch_ua = build_sec_ch_ua(browser, os, chrome_major);
        let sec_ch_ua_mobile = Some(match device_class {
            DeviceClass::Mobile | DeviceClass::Tablet => "?1".to_string(),
            DeviceClass::Desktop => "?0".to_string(),
        });
        let sec_ch_ua_platform = Some(format!("\"{}\"", platform_string(os)));
        let viewport = random_viewport(device_class);
        let device_pixel_ratio = match device_class {
            DeviceClass::Desktop => *[1.0f32, 1.25, 1.5, 2.0]
                .choose(&mut rng)
                .expect("dpr desktop array non-empty"),
            DeviceClass::Tablet => *[1.0f32, 2.0]
                .choose(&mut rng)
                .expect("dpr tablet array non-empty"),
            DeviceClass::Mobile => *[2.0f32, 2.625, 3.0]
                .choose(&mut rng)
                .expect("dpr mobile array non-empty"),
        };
        let timezone = TIMEZONES
            .choose(&mut rng)
            .expect("TIMEZONES array non-empty")
            .to_string();
        let hardware_concurrency = *[2u8, 4, 6, 8, 10, 12, 16]
            .choose(&mut rng)
            .expect("concurrency array non-empty");
        let device_memory = *[2u8, 4, 6, 8, 16, 32]
            .choose(&mut rng)
            .expect("memory array non-empty");
        let dnt = rng.random::<bool>();

        let id = format!(
            "{}-{}-{}-v{}",
            browser_string(browser),
            os_string(os),
            device_class_string(device_class),
            rng.random::<u32>()
        );

        Self {
            id,
            browser,
            os,
            device_class,
            user_agent,
            accept,
            accept_language,
            accept_encoding,
            sec_ch_ua,
            sec_ch_ua_mobile,
            sec_ch_ua_platform,
            viewport,
            device_pixel_ratio,
            timezone,
            hardware_concurrency,
            device_memory,
            dnt,
        }
    }

    /// Apply profile headers to a reqwest request builder.
    pub fn apply_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut b = builder
            .header("user-agent", &self.user_agent)
            .header("accept", &self.accept)
            .header("accept-language", &self.accept_language)
            .header("accept-encoding", &self.accept_encoding)
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-site", "none")
            .header("upgrade-insecure-requests", "1");

        if let Some(ref v) = self.sec_ch_ua {
            b = b.header("sec-ch-ua", v);
        }
        if let Some(ref v) = self.sec_ch_ua_mobile {
            b = b.header("sec-ch-ua-mobile", v);
        }
        if let Some(ref v) = self.sec_ch_ua_platform {
            b = b.header("sec-ch-ua-platform", v);
        }
        if self.dnt {
            b = b.header("dnt", "1");
        }
        b
    }

    /// JavaScript snippet to inject into a headless browser to match this profile.
    pub fn stealth_script(&self) -> String {
        format!(
            r#"
            Object.defineProperty(navigator, 'webdriver', {{
                get: () => undefined,
            }});
            Object.defineProperty(navigator, 'plugins', {{
                get: () => [1, 2, 3, 4, 5],
            }});
            Object.defineProperty(navigator, 'languages', {{
                get: () => ['{}', 'en'],
            }});
            Object.defineProperty(screen, 'width', {{ get: () => {} }});
            Object.defineProperty(screen, 'height', {{ get: () => {} }});
            Object.defineProperty(window, 'devicePixelRatio', {{ get: () => {} }});
            Object.defineProperty(navigator, 'hardwareConcurrency', {{ get: () => {} }});
            Object.defineProperty(navigator, 'deviceMemory', {{ get: () => {} }});
            "#,
            self.accept_language.split(',').next().unwrap_or("en-US"),
            self.viewport.width,
            self.viewport.height,
            self.device_pixel_ratio,
            self.hardware_concurrency,
            self.device_memory,
        )
    }
}

// --- Random builders ------------------------------------------------------

pub fn build_user_agent(
    browser: Browser,
    os: Os,
    device_class: DeviceClass,
    chrome_major: u16,
    firefox_major: u16,
    safari_major: u16,
) -> String {
    match (browser, os, device_class) {
        (Browser::Chrome, Os::Windows, _) => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            chrome_major
        ),
        (Browser::Chrome, Os::MacOs, _) => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            chrome_major
        ),
        (Browser::Chrome, Os::Linux, _) => format!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            chrome_major
        ),
        (Browser::Chrome, Os::Android, DeviceClass::Mobile) => format!(
            "Mozilla/5.0 (Linux; Android {}; {}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Mobile Safari/537.36",
            android_version(),
            random_phone_model(),
            chrome_major
        ),
        (Browser::Chrome, Os::Android, DeviceClass::Tablet) => format!(
            "Mozilla/5.0 (Linux; Android {}; {}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            android_version(),
            random_tablet_model(),
            chrome_major
        ),
        (Browser::Chrome, Os::Ios, _) => format!(
            "Mozilla/5.0 (iPhone; CPU iPhone OS {} like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/{}.0.0.0 Mobile/15E148 Safari/604.1",
            ios_version(),
            chrome_major
        ),
        (Browser::Firefox, Os::Windows, _) => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{}.0) Gecko/20100101 Firefox/{}.0",
            firefox_major, firefox_major
        ),
        (Browser::Firefox, Os::MacOs, _) => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:{}.0) Gecko/20100101 Firefox/{}.0",
            firefox_major, firefox_major
        ),
        (Browser::Firefox, Os::Linux, _) => format!(
            "Mozilla/5.0 (X11; Linux x86_64; rv:{}.0) Gecko/20100101 Firefox/{}.0",
            firefox_major, firefox_major
        ),
        (Browser::Firefox, Os::Android, _) => format!(
            "Mozilla/5.0 (Android {}; Mobile; rv:{}.0) Gecko/{}.0 Firefox/{}.0",
            android_version(),
            firefox_major,
            firefox_major,
            firefox_major
        ),
        (Browser::Safari, Os::MacOs, _) => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{}.{} Safari/605.1.15",
            safari_major,
            rand::rng().random_range(0u16..=5)
        ),
        (Browser::Safari, Os::Ios, DeviceClass::Mobile) => format!(
            "Mozilla/5.0 (iPhone; CPU iPhone OS {} like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{}.{} Mobile/15E148 Safari/604.1",
            ios_version(),
            safari_major,
            rand::rng().random_range(0u16..=5)
        ),
        (Browser::Safari, Os::Ios, DeviceClass::Tablet) => format!(
            "Mozilla/5.0 (iPad; CPU OS {} like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{}.{} Mobile/15E148 Safari/604.1",
            ios_version(),
            safari_major,
            rand::rng().random_range(0u16..=5)
        ),
        (Browser::Edge, Os::Windows, _) => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36 Edg/{}.0.0.0",
            chrome_major,
            chrome_major - 2
        ),
        (Browser::Edge, Os::MacOs, _) => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36 Edg/{}.0.0.0",
            chrome_major,
            chrome_major - 2
        ),
        // Fallback to Chrome on any unsupported combo.
        _ => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            chrome_major
        ),
    }
}

fn build_accept(browser: Browser) -> String {
    match browser {
        Browser::Firefox => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
                .to_string()
        }
        _ => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"
                .to_string()
        }
    }
}

fn build_accept_language() -> String {
    let locales = [
        "en-US,en;q=0.9",
        "en-GB,en;q=0.9",
        "en-US,en;q=0.8,fr;q=0.5",
        "en-CA,en;q=0.9,fr-CA;q=0.5",
        "de-DE,de;q=0.9,en;q=0.5",
        "fr-FR,fr;q=0.9,en;q=0.5",
        "es-ES,es;q=0.9,en;q=0.5",
        "ja-JP,ja;q=0.9,en;q=0.5",
        "zh-CN,zh;q=0.9,en;q=0.5",
    ];
    locales
        .choose(&mut rand::rng())
        .expect("locales array non-empty")
        .to_string()
}

fn build_accept_encoding(browser: Browser) -> String {
    match browser {
        Browser::Safari => "gzip, deflate, br".to_string(),
        _ => "gzip, deflate, br, zstd".to_string(),
    }
}

fn build_sec_ch_ua(browser: Browser, _os: Os, chrome_major: u16) -> Option<String> {
    match browser {
        Browser::Chrome | Browser::Edge => {
            let brand = if browser == Browser::Edge {
                "Microsoft Edge"
            } else {
                "Google Chrome"
            };
            Some(format!(
                "\"Chromium\";v=\"{}\", \"Not;A=Brand\";v=\"24\", \"{}\";v=\"{}\"",
                chrome_major, brand, chrome_major
            ))
        }
        _ => None,
    }
}

fn random_viewport(device_class: DeviceClass) -> Viewport {
    let mut rng = rand::rng();
    match device_class {
        DeviceClass::Desktop => {
            let (w, h) = *[
                (1920u16, 1080u16),
                (2560, 1440),
                (1440, 900),
                (1536, 864),
                (1366, 768),
                (1680, 1050),
                (1280, 720),
            ]
            .choose(&mut rng)
            .expect("desktop viewport array non-empty");
            Viewport {
                width: w,
                height: h,
            }
        }
        DeviceClass::Tablet => {
            let (w, h) = *[(1024u16, 1366u16), (834, 1194), (810, 1080), (768, 1024)]
                .choose(&mut rng)
                .expect("tablet viewport array non-empty");
            Viewport {
                width: w,
                height: h,
            }
        }
        DeviceClass::Mobile => {
            let (w, h) = *[
                (393u16, 852u16),
                (360, 800),
                (412, 915),
                (390, 844),
                (414, 896),
                (375, 812),
            ]
            .choose(&mut rng)
            .expect("mobile viewport array non-empty");
            Viewport {
                width: w,
                height: h,
            }
        }
    }
}

fn android_version() -> String {
    let v = ["12", "13", "14", "15"]
        .choose(&mut rand::rng())
        .expect("android versions non-empty");
    v.to_string()
}

fn ios_version() -> String {
    let mut rng = rand::rng();
    format!(
        "{}.{}.{}",
        rng.random_range(15u16..=17),
        rng.random_range(0u16..=6),
        rng.random_range(0u16..=3)
    )
}

fn random_phone_model() -> String {
    let models = [
        "SM-S928B",
        "SM-S921B",
        "Pixel 8 Pro",
        "Pixel 8",
        "Pixel 7",
        "SM-G996B",
        "SM-A546B",
        "Redmi Note 13",
        "OnePlus 12",
    ];
    models
        .choose(&mut rand::rng())
        .expect("phone models non-empty")
        .to_string()
}

fn random_tablet_model() -> String {
    let models = ["SM-X910", "SM-X810", "iPad", "Lenovo TB350FU"];
    models
        .choose(&mut rand::rng())
        .expect("tablet models non-empty")
        .to_string()
}

fn browser_string(b: Browser) -> &'static str {
    match b {
        Browser::Chrome => "chrome",
        Browser::Firefox => "firefox",
        Browser::Safari => "safari",
        Browser::Edge => "edge",
    }
}

fn os_string(o: Os) -> &'static str {
    match o {
        Os::Windows => "win",
        Os::MacOs => "mac",
        Os::Linux => "linux",
        Os::Android => "android",
        Os::Ios => "ios",
    }
}

fn device_class_string(d: DeviceClass) -> &'static str {
    match d {
        DeviceClass::Desktop => "desktop",
        DeviceClass::Mobile => "mobile",
        DeviceClass::Tablet => "tablet",
    }
}

fn platform_string(o: Os) -> &'static str {
    match o {
        Os::Windows => "Windows",
        Os::MacOs => "macOS",
        Os::Linux => "Linux",
        Os::Android => "Android",
        Os::Ios => "iOS",
    }
}

/// Binds a device profile to a proxy and domain for the lifetime of a session.
#[derive(Debug, Clone)]
pub struct Session {
    pub domain: String,
    pub profile: DeviceProfile,
    pub proxy_url: Option<String>,
}

/// Session manager that keeps (domain → Session) sticky mappings.
#[derive(Debug)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Session>>,
    sticky: bool,
}

/// Whether sessions should stick to the same UA+proxy for a given domain.
#[derive(Clone, Copy)]
pub enum StickySessions {
    Sticky,
    PerRequest,
}

impl SessionManager {
    pub fn new(sticky: StickySessions) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            sticky: match sticky {
                StickySessions::Sticky => true,
                StickySessions::PerRequest => false,
            },
        }
    }

    /// Get or create a session for a domain.
    pub fn session_for(&self, domain: &str, proxy_url: Option<String>) -> Session {
        if self.sticky {
            if let Some(session) = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(domain)
            {
                return session.clone();
            }
        }

        let profile = DeviceProfile::random();
        let session = Session {
            domain: domain.to_string(),
            profile,
            proxy_url: proxy_url.clone(),
        };

        if self.sticky {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(domain.to_string(), session.clone());
        }
        session
    }

    pub fn clear(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(StickySessions::Sticky)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_profile_variety() {
        let profiles: Vec<_> = (0..20).map(|_| DeviceProfile::random()).collect();
        assert!(!profiles.is_empty());
        let first = &profiles[0].user_agent;
        assert!(
            profiles.iter().any(|p| p.user_agent != *first),
            "random profiles should vary"
        );
    }

    #[test]
    fn test_session_sticky() {
        let mgr = SessionManager::new(StickySessions::Sticky);
        let s1 = mgr.session_for("example.com", Some("http://p1".to_string()));
        let s2 = mgr.session_for("example.com", Some("http://p2".to_string()));
        assert_eq!(s1.profile.user_agent, s2.profile.user_agent);
    }

    #[test]
    fn test_session_non_sticky() {
        let mgr = SessionManager::new(StickySessions::PerRequest);
        let s1 = mgr.session_for("example.com", None);
        let s2 = mgr.session_for("example.com", None);
        assert_eq!(s1.domain, s2.domain);
    }
}
