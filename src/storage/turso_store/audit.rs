use async_trait::async_trait;
use super::TursoStore;
use crate::engine::fingerprint::{Fingerprint, FingerprintAuditLog};
use chrono::Utc;
use libsql::params;


#[async_trait]
impl FingerprintAuditLog for TursoStore {
    async fn log_fingerprint_use(
        &self,
        fp: &Fingerprint,
        status_code: Option<u16>,
        error: Option<&str>,
    ) {
        let used_at = Utc::now().to_rfc3339();
        let working = error.is_none() && status_code.map(|s| s < 400).unwrap_or(true);

        // Append an audit row.
        let log = self
            .conn
            .execute(
                r#"
                INSERT INTO fingerprint_log
                    (fingerprint_id, ip, isp, asn, country, user_agent, accept_language,
                     device_class, geo_region, status_code, error, used_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    fp.id.as_str(),
                    fp.ip.as_str(),
                    fp.isp.name.as_str(),
                    fp.isp.asn.as_str(),
                    fp.isp.country.as_str(),
                    fp.user_agent.as_str(),
                    fp.accept_language.as_str(),
                    format!("{:?}", fp.device_class),
                    format!("{:?}", fp.geo_region),
                    status_code.map(|s| s as i64),
                    error,
                    used_at.as_str(),
                ],
            )
            .await;
        if let Err(e) = log {
            tracing::error!("failed to log fingerprint use: {}", e);
        }

        // Upsert the running health row for this IP.
        let health = self
            .conn
            .execute(
                r#"
                INSERT INTO ip_health
                    (ip, fingerprint_id, isp, asn, country, user_agent, accept_language,
                     device_class, geo_region, working, success_count, failure_count,
                     last_used, last_error)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14)
                ON CONFLICT(ip) DO UPDATE SET
                    fingerprint_id  = excluded.fingerprint_id,
                    isp             = excluded.isp,
                    asn             = excluded.asn,
                    country         = excluded.country,
                    user_agent      = excluded.user_agent,
                    accept_language = excluded.accept_language,
                    device_class    = excluded.device_class,
                    geo_region      = excluded.geo_region,
                    working         = excluded.working,
                    success_count   = ip_health.success_count + excluded.success_count,
                    failure_count   = ip_health.failure_count + excluded.failure_count,
                    last_used       = excluded.last_used,
                    last_error      = excluded.last_error
                "#,
                params![
                    fp.ip.as_str(),
                    fp.id.as_str(),
                    fp.isp.name.as_str(),
                    fp.isp.asn.as_str(),
                    fp.isp.country.as_str(),
                    fp.user_agent.as_str(),
                    fp.accept_language.as_str(),
                    format!("{:?}", fp.device_class),
                    format!("{:?}", fp.geo_region),
                    working as i64,
                    (if working { 1_i64 } else { 0_i64 }),
                    (if working { 0_i64 } else { 1_i64 }),
                    used_at.as_str(),
                    error,
                ],
            )
            .await;
        if let Err(e) = health {
            tracing::error!("failed to upsert ip health: {}", e);
        }
    }
}
