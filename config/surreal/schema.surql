-- webfind SurrealDB schema
-- Graph + structured storage for pages, domains, and relationships

-- ============================================================
-- TABLES
-- ============================================================

DEFINE TABLE page SCHEMAFULL;
DEFINE FIELD url ON page TYPE string;
DEFINE FIELD final_url ON page TYPE string;
DEFINE FIELD title ON page TYPE string;
DEFINE FIELD description ON page TYPE option<string>;
DEFINE FIELD content_text ON page TYPE string;
DEFINE FIELD content_markdown ON page TYPE option<string>;
DEFINE FIELD excerpt ON page TYPE string;
DEFINE FIELD domain ON page TYPE string;
DEFINE FIELD language ON page TYPE string;
DEFINE FIELD language_confidence ON page TYPE float;
DEFINE FIELD published_at ON page TYPE option<datetime>;
DEFINE FIELD crawled_at ON page TYPE datetime;
DEFINE FIELD fetched_at ON page TYPE datetime;
DEFINE FIELD author ON page TYPE option<string>;
DEFINE FIELD site_name ON page TYPE option<string>;
DEFINE FIELD word_count ON page TYPE u32;
DEFINE FIELD reading_time_seconds ON page TYPE u32;
DEFINE FIELD reading_ease ON page TYPE float;
DEFINE FIELD grade_level ON page TYPE float;
DEFINE FIELD schema_type ON page TYPE option<string>;
DEFINE FIELD is_valid_content ON page TYPE bool;
DEFINE FIELD html_size_bytes ON page TYPE u64;
DEFINE FIELD page_rank ON page TYPE float VALUE $value OR 0.0;
DEFINE FIELD status_code ON page TYPE u16;

DEFINE TABLE domain_node SCHEMAFULL;
DEFINE FIELD name ON domain_node TYPE string;
DEFINE FIELD page_count ON domain_node TYPE u32 VALUE $value OR 0;
DEFINE FIELD avg_authority ON domain_node TYPE float VALUE $value OR 0.0;
DEFINE FIELD first_crawled ON domain_node TYPE option<datetime>;
DEFINE FIELD last_crawled ON domain_node TYPE option<datetime>;

-- ============================================================
-- EDGE TABLES
-- ============================================================

DEFINE TABLE found SCHEMAFULL;
DEFINE FIELD source ON found TYPE record<page>;
DEFINE FIELD target ON found TYPE record<page>;
DEFINE FIELD anchor_text ON found TYPE option<string>;
DEFINE FIELD crawled_at ON found TYPE datetime;

DEFINE TABLE links_to SCHEMAFULL;
DEFINE FIELD source ON links_to TYPE record<page>;
DEFINE FIELD target ON links_to TYPE record<page>;
DEFINE FIELD anchor_text ON links_to TYPE option<string>;
DEFINE FIELD crawled_at ON links_to TYPE datetime;

DEFINE TABLE belongs_to SCHEMAFULL;
DEFINE FIELD page ON belongs_to TYPE record<page>;
DEFINE FIELD domain ON belongs_to TYPE record<domain_node>;

-- ============================================================
-- INDEXES
-- ============================================================

DEFINE INDEX page_url_unique ON page FIELDS url UNIQUE;
DEFINE INDEX page_domain_idx ON page FIELDS domain;
DEFINE INDEX page_language_idx ON page FIELDS language;
DEFINE INDEX page_crawled_idx ON page FIELDS crawled_at;
DEFINE INDEX page_page_rank_idx ON page FIELDS page_rank;

DEFINE INDEX domain_name_unique ON domain_node FIELDS name UNIQUE;

DEFINE INDEX found_source_idx ON found FIELDS source;
DEFINE INDEX found_target_idx ON found FIELDS target;

DEFINE INDEX links_to_source_idx ON links_to FIELDS source;
DEFINE INDEX links_to_target_idx ON links_to FIELDS target;

DEFINE INDEX belongs_to_page_idx ON belongs_to FIELDS page;
DEFINE INDEX belongs_to_domain_idx ON belongs_to FIELDS domain;

-- ============================================================
-- FINGERPRINT AUDIT
-- ============================================================

DEFINE TABLE ip_health SCHEMAFULL;
DEFINE FIELD ip               ON ip_health TYPE string  ASSERT $value != NONE;
DEFINE FIELD fingerprint_id   ON ip_health TYPE string;
DEFINE FIELD isp              ON ip_health TYPE string;
DEFINE FIELD asn              ON ip_health TYPE string;
DEFINE FIELD country          ON ip_health TYPE string;
DEFINE FIELD user_agent       ON ip_health TYPE string;
DEFINE FIELD accept_language  ON ip_health TYPE string;
DEFINE FIELD device_class     ON ip_health TYPE string;
DEFINE FIELD geo_region       ON ip_health TYPE string;
DEFINE FIELD working          ON ip_health TYPE bool   DEFAULT true;
DEFINE FIELD success_count    ON ip_health TYPE number DEFAULT 0;
DEFINE FIELD failure_count    ON ip_health TYPE number DEFAULT 0;
DEFINE FIELD last_used        ON ip_health TYPE option<datetime>;
DEFINE FIELD last_error       ON ip_health TYPE option<string>;
DEFINE FIELD discarded_at     ON ip_health TYPE option<datetime>;
DEFINE INDEX ip_health_ip_unique ON ip_health FIELDS ip UNIQUE;

DEFINE TABLE fingerprint_log SCHEMAFULL;
DEFINE FIELD fingerprint_id   ON fingerprint_log TYPE string;
DEFINE FIELD ip               ON fingerprint_log TYPE string;
DEFINE FIELD isp              ON fingerprint_log TYPE string;
DEFINE FIELD asn              ON fingerprint_log TYPE string;
DEFINE FIELD country          ON fingerprint_log TYPE string;
DEFINE FIELD user_agent       ON fingerprint_log TYPE string;
DEFINE FIELD accept_language  ON fingerprint_log TYPE string;
DEFINE FIELD device_class     ON fingerprint_log TYPE string;
DEFINE FIELD geo_region       ON fingerprint_log TYPE string;
DEFINE FIELD status_code      ON fingerprint_log TYPE option<number>;
DEFINE FIELD error            ON fingerprint_log TYPE option<string>;
DEFINE FIELD used_at          ON fingerprint_log TYPE datetime;
DEFINE INDEX fingerprint_log_ip_idx ON fingerprint_log FIELDS ip;
DEFINE INDEX fingerprint_log_used_at_idx ON fingerprint_log FIELDS used_at;

-- ============================================================
-- FULL-TEXT SEARCH
-- ============================================================

DEFINE ANALYZER webfind_analyzer TOKENIZERS blank, class FILTERS lowercase, snowball(english);

DEFINE INDEX page_fulltext ON page FIELDS title, content_text, excerpt SEARCH ANALYZER webfind_analyzer BM25;

-- ============================================================
-- PERMISSIONS
-- ============================================================

DEFINE PERMISSIONS FOR page, domain_node, found, links_to, belongs_to, ip_health, fingerprint_log
  SELECT WHERE true
  CREATE WHERE true
  UPDATE WHERE true
  DELETE WHERE true;
