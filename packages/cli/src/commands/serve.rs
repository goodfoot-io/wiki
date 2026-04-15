use std::convert::Infallible;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Router, serve};
use miette::{IntoDiagnostic, Result, WrapErr};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::sync::broadcast;
use tokio::task;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::index::{SearchResult, WikiIndex};
use crate::render::{RenderMode, render_html, wrap_page};

#[derive(Clone)]
struct AppState {
    reload_tx: broadcast::Sender<()>,
    live_reload: bool,
    index: Arc<Mutex<WikiIndex>>,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: Option<String>,
}

pub fn run(port: u16, no_reload: bool, repo_root: &Path) -> Result<i32> {
    let runtime = RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()
        .wrap_err("failed to create runtime for wiki serve")?;

    runtime.block_on(async_run(port, no_reload, repo_root))
}

async fn async_run(port: u16, no_reload: bool, repo_root: &Path) -> Result<i32> {
    let repo_root = repo_root.to_path_buf();

    let index = {
        let root = repo_root.clone();
        task::spawn_blocking(move || WikiIndex::prepare(&root))
            .await
            .into_diagnostic()
            .wrap_err("failed to spawn wiki index build")??
    };
    let index = Arc::new(Mutex::new(index));

    let (reload_tx, _) = broadcast::channel(64);
    let _watcher = if no_reload {
        None
    } else {
        Some(start_watcher(&repo_root, reload_tx.clone(), index.clone())?)
    };

    let state = AppState {
        reload_tx,
        live_reload: !no_reload,
        index,
    };

    let app = Router::new()
        .route("/", get(handler_index))
        .route("/search", get(handler_search))
        .route("/_sse", get(handler_sse))
        .route("/*title", get(handler_page))
        .with_state(state);

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to bind wiki server on port {port}"))?;
    println!("Serving wiki on http://0.0.0.0:{port}");

    serve(listener, app)
        .await
        .into_diagnostic()
        .wrap_err("wiki server exited unexpectedly")?;
    Ok(0)
}

fn start_watcher(
    repo_root: &Path,
    reload_tx: broadcast::Sender<()>,
    index: Arc<Mutex<WikiIndex>>,
) -> Result<RecommendedWatcher> {
    let repo_root = repo_root.to_path_buf();
    let callback_repo_root = repo_root.clone();
    let worker_repo_root = repo_root.clone();
    let (path_tx, path_rx) = mpsc::channel::<std::path::PathBuf>();

    thread::spawn(move || watcher_worker(worker_repo_root, reload_tx, index, path_rx));

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        for path in event.paths {
            if is_watchable_wiki_path(&path, &callback_repo_root) {
                let _ = path_tx.send(path);
            }
        }
    })
    .into_diagnostic()
    .wrap_err("failed to create wiki file watcher")?;

    watcher
        .watch(&repo_root, RecursiveMode::Recursive)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to watch {}", repo_root.display()))?;

    Ok(watcher)
}

fn watcher_worker(
    repo_root: std::path::PathBuf,
    reload_tx: broadcast::Sender<()>,
    index: Arc<Mutex<WikiIndex>>,
    path_rx: mpsc::Receiver<std::path::PathBuf>,
) {
    let debounce = Duration::from_millis(100);

    loop {
        let first_path = match path_rx.recv() {
            Ok(path) => path,
            Err(_) => return,
        };

        let mut batch = std::collections::HashSet::from([first_path]);
        while let Ok(path) = path_rx.recv_timeout(debounce) {
            batch.insert(path);
        }

        let mut changed_paths = batch.into_iter().collect::<Vec<_>>();
        changed_paths.sort();

        if let Ok(mut guard) = index.lock() {
            match guard.refresh_paths(&changed_paths) {
                Ok(()) => {
                    let _ = reload_tx.send(());
                }
                Err(error) => {
                    eprintln!(
                        "wiki: failed to refresh index after file change batch in {}: {error}",
                        repo_root.display()
                    );
                }
            }
        }
    }
}

fn is_watchable_wiki_path(path: &Path, repo_root: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return false;
    }

    let Ok(relative) = path.strip_prefix(repo_root) else {
        return false;
    };
    let relative = relative.to_string_lossy();
    if relative.ends_with(".wiki.md") {
        return true;
    }

    let wiki_dir = std::env::var("WIKI_DIR").unwrap_or_else(|_| "wiki".to_string());
    relative.starts_with(&wiki_dir)
}

async fn handler_index(State(state): State<AppState>) -> Response {
    let pages = match load_index_pages(state.index).await {
        Ok(pages) => pages,
        Err(error) => return html_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let mut body =
        String::from("<section class=\"panel\"><h1>All pages</h1><ul class=\"page-list\">");
    for page in pages {
        body.push_str(&format!(
            "<li><a href=\"/{href}\">{title}</a><br><span class=\"search-meta\">{summary}</span></li>",
            href = percent_encode_title(&page.title),
            title = escape_html(&page.title),
            summary = escape_html(&page.summary),
        ));
    }
    body.push_str("</ul></section>");

    Html(wrap_page("All pages", &body, state.live_reload)).into_response()
}

async fn handler_search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Response {
    let query = params.q.unwrap_or_default();

    let mut body = format!(
        "<section class=\"panel\"><h1>Search</h1><p class=\"search-meta\">Query: <code>{}</code></p>",
        escape_html(&query)
    );
    if query.trim().is_empty() {
        body.push_str("<p>Enter a search term.</p>");
    } else {
        match search_pages(state.index, query.clone()).await {
            Ok(results) if results.is_empty() => body.push_str("<p>No matching pages found.</p>"),
            Ok(results) => body.push_str(&render_search_results(&results)),
            Err(error) => return html_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
        }
    }
    body.push_str("</section>");

    Html(wrap_page("Search", &body, state.live_reload)).into_response()
}

async fn handler_page(
    State(state): State<AppState>,
    AxumPath(title): AxumPath<String>,
) -> Response {
    let requested = title.trim_matches('/');

    match render_page_response(state.index, requested.to_string()).await {
        Ok(PageResponse::Found { title, body }) => {
            Html(wrap_page(&title, &body, state.live_reload)).into_response()
        }
        Ok(PageResponse::NotFound {
            requested,
            suggestions,
        }) => {
            let body = render_not_found_html(&requested, &suggestions);
            (
                StatusCode::NOT_FOUND,
                Html(wrap_page("Page not found", &body, state.live_reload)),
            )
                .into_response()
        }
        Err(error) => html_error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn handler_sse(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>> {
    let stream =
        BroadcastStream::new(state.reload_tx.subscribe()).filter_map(|message| match message {
            Ok(()) => Some(Ok(Event::default().data("reload"))),
            Err(_) => None,
        });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn render_search_results(results: &[SearchResult]) -> String {
    let mut body = String::from("<ul class=\"page-list\">");
    for result in results {
        body.push_str(&format!(
            "<li><a href=\"/{href}\">{title}</a><br><span class=\"search-meta\">{summary}</span></li>",
            href = percent_encode_title(&result.title),
            title = escape_html(&result.title),
            summary = escape_html(&result.summary),
        ));
    }
    body.push_str("</ul>");
    body
}

fn render_not_found_html(requested: &str, suggestions: &[SearchResult]) -> String {
    let mut body = format!(
        "<section class=\"panel\"><h1>Page not found</h1><p>No page matched <code>{}</code>.</p>",
        escape_html(requested)
    );
    if !suggestions.is_empty() {
        body.push_str("<p>Suggestions:</p>");
        body.push_str(&render_search_results(suggestions));
    }
    body.push_str("</section>");
    body
}

fn html_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Html(wrap_page(
            "Error",
            &format!(
                "<section class=\"panel\"><h1>Error</h1><p>{}</p></section>",
                escape_html(message)
            ),
            false,
        )),
    )
        .into_response()
}

fn percent_encode_title(title: &str) -> String {
    let mut encoded = String::new();
    for byte in title.bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

enum PageResponse {
    Found {
        title: String,
        body: String,
    },
    NotFound {
        requested: String,
        suggestions: Vec<SearchResult>,
    },
}

async fn load_index_pages(
    index: Arc<Mutex<WikiIndex>>,
) -> std::result::Result<Vec<crate::index::PageListEntry>, String> {
    task::spawn_blocking(move || {
        let guard = index.lock().map_err(|e| e.to_string())?;
        guard.list_pages(None).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn search_pages(
    index: Arc<Mutex<WikiIndex>>,
    query: String,
) -> std::result::Result<Vec<SearchResult>, String> {
    task::spawn_blocking(move || {
        let guard = index.lock().map_err(|e| e.to_string())?;
        guard.search(&query).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn render_page_response(
    index: Arc<Mutex<WikiIndex>>,
    requested: String,
) -> std::result::Result<PageResponse, String> {
    task::spawn_blocking(move || {
        let guard = index.lock().map_err(|e| e.to_string())?;
        match guard.resolve_page(&requested).map_err(|e| e.to_string())? {
            Some(page) => {
                let body = render_html(&page.content, RenderMode::FullPage, &guard);
                Ok(PageResponse::Found {
                    title: page.title,
                    body,
                })
            }
            None => {
                let suggestions = guard.suggest(&requested).unwrap_or_default();
                Ok(PageResponse::NotFound {
                    requested,
                    suggestions,
                })
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}
