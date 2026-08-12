use askama::Template;

/// Home page with the big centered search box.
#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub query: String,
    pub seed: String,
}

/// Search results page.
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub query: String,
    pub max_pages: u32,
    pub seed: String,
    pub categories: Vec<String>,
    pub categories_str: String,
    pub active_type: String,
    pub is_all_type: bool,
    pub cached: bool,
    pub results: Vec<crate::schema::response::SearchResult>,
}

/// Inline result list fragment (used for cached results and SSE final result).
#[derive(Template)]
#[template(path = "result_list.html")]
pub struct ResultListTemplate<'a> {
    pub query: &'a str,
    pub results: &'a [crate::schema::response::SearchResult],
}

/// Search suggestions dropdown fragment.
#[derive(Template)]
#[template(path = "suggestions.html")]
pub struct SuggestionsTemplate {
    pub suggestions: Vec<String>,
}

/// Category sidebar fragment (list of categories with counts).
#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub categories: Vec<crate::engine::categories::Category>,
    pub current_query: String,
}

/// About page.
#[derive(Template)]
#[template(path = "about.html")]
pub struct AboutTemplate;

/// Error page.
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub message: String,
}

/// SSE progress payload rendered as a small HTML swap.
#[derive(Template)]
#[template(path = "research_progress.html", ext = "html")]
pub struct ResearchProgressTemplate<'a> {
    pub query: &'a str,
}
