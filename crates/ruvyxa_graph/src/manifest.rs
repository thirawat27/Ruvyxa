use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ruvyxa_diagnostics::{Result, RuvyxaError};
use serde::{Deserialize, Serialize};

/// Route parameters passed from the matcher to page and API renderers.
///
/// Values are JSON-shaped because catch-all segments are arrays while an
/// omitted optional catch-all has no entry.
pub type RouteParams = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteManifest {
    pub app_dir: PathBuf,
    pub routes: Vec<RouteEntry>,
    /// Optional file-system locale routing policy copied from project config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n: Option<I18nRouting>,
}

/// Validated locale-routing policy shared by discovery, native serving, and
/// deployment runtimes. Validation belongs to the config boundary; consumers
/// can therefore use these values without interpreting raw user input again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct I18nRouting {
    pub locales: Vec<String>,
    pub default_locale: String,
    pub locale_param: String,
    pub detect_locale: bool,
    pub cookie: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub id: String,
    pub path: String,
    pub kind: RouteKind,
    pub file: PathBuf,
    pub layout_chain: Vec<String>,
    /// `template.tsx` files on the path to this route, root first.
    ///
    /// Separate from `layout_chain` rather than merged into it because a level
    /// may have either, both, or neither, and composition interleaves them by
    /// directory.
    #[serde(default)]
    pub template_chain: Vec<String>,
    /// Parallel-route slots this route composes into its layouts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<RouteSlot>,
    /// Interceptions reachable from this route, by the URL each one covers.
    ///
    /// Carried on the route the user is *standing on* rather than on the route
    /// being intercepted, because that is the bundle that has to be able to
    /// render the overlay without a round trip. The intercepted route keeps its
    /// own entry untouched, which is what makes a hard load show the real page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intercepts: Vec<RouteIntercept>,
    pub server_modules: Vec<String>,
    pub client_modules: Vec<String>,
    pub runtime: RuntimeTarget,
    /// Rendering strategy and metadata for this route.
    #[serde(default)]
    pub render: RenderMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteKind {
    Page,
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeTarget {
    Node,
    Edge,
    Static,
}

/// Per-route rendering strategy — determines when and how the HTML is generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RenderStrategy {
    /// Server-Side Rendering: HTML generated on every request (default).
    #[default]
    Ssr,
    /// Static Site Generation: HTML pre-rendered at build time.
    Ssg,
    /// Incremental Static Regeneration: pre-rendered at build time, revalidated
    /// in the background after a TTL expires.
    Isr,
    /// Client-Side Rendering: minimal shell HTML served, full rendering happens
    /// in the browser via hydration without server-rendered content.
    Csr,
    /// Partial Pre-Rendering: static shell pre-rendered at build time with
    /// dynamic "holes" that stream in at request time.
    Ppr,
}

/// When a server-rendered route downloads and starts its client runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HydrationMode {
    /// Load and hydrate as soon as the document parser reaches the module.
    #[default]
    Load,
    /// Download the route bundle when the browser is idle.
    Idle,
    /// Download the route bundle when the document becomes visible.
    Visible,
    /// Ship no client bundle for this route.
    None,
}

/// Metadata that controls the rendering strategy for a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMeta {
    /// The rendering strategy for this route.
    pub strategy: RenderStrategy,
    /// ISR revalidation interval in seconds (only meaningful for `Isr`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revalidate: Option<u64>,
    /// Whether the page exports `getStaticParams` or `staticParams` for dynamic SSG routes.
    #[serde(default)]
    pub has_static_params: bool,
    /// Static paths discovered from `getStaticParams` at build time.
    /// Empty until the build phase populates them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub static_paths: Vec<String>,
    /// For PPR: whether the page uses `<Suspense>` boundaries that mark
    /// dynamic slots to be streamed at request time.
    #[serde(default)]
    pub has_dynamic_slots: bool,
    /// When the client bundle is scheduled, and whether there is one at all.
    ///
    /// This is the single source of truth for client-side scheduling.
    /// `export const hydrate = false` (or `'none'`) parses to
    /// [`HydrationMode::None`], which is what
    /// [`RenderMeta::ships_client_bundle`] answers from — a page that ships no
    /// bundle runs no interactivity, so `'use client'` islands do not execute
    /// there. A separate `hydrate: bool` field used to be stored beside this
    /// one and was only ever `hydration != None`; because both were public and
    /// independently assignable, callers could and did set one without the
    /// other, leaving the bundler and the document writer disagreeing about
    /// whether a route had JavaScript.
    #[serde(default)]
    pub hydration: HydrationMode,
    /// `export const serverComponents = true`: render this route through the
    /// React Server Components pipeline.
    ///
    /// Orthogonal to [`Self::strategy`] rather than a variant of it, because it
    /// answers a different question. The strategy decides *when* a route is
    /// rendered — at build time, per request, or on a revalidation interval —
    /// while this decides *which two graphs* render it. A server-components
    /// route can still be SSG or ISR, and folding the two would have made
    /// `revalidate` and this mutually exclusive for no reason.
    ///
    /// Opt-in per route rather than a project-wide default: turning it on
    /// changes what reaches the browser for that route, and a framework-wide
    /// switch would change every existing page at once.
    #[serde(default)]
    pub server_components: bool,
    /// `export const dynamic = 'force-dynamic'`: this route asked to be
    /// rendered per request.
    ///
    /// Distinct from an ordinary SSR strategy, which is only the *default*.
    /// Reading the export used to decide one thing — do not pre-render this —
    /// and nothing downstream could tell the two apart afterwards, so the
    /// runtime render cache stored the document and served it unchanged for the
    /// life of the process. The page asked for the opposite of that, and Next,
    /// whose convention this is, also takes it to mean "do not cache".
    #[serde(default)]
    pub force_dynamic: bool,
}

impl RenderMeta {
    /// Whether the served HTML includes a client bundle.
    pub fn ships_client_bundle(&self) -> bool {
        self.hydration != HydrationMode::None
    }
}

impl Default for RenderMeta {
    fn default() -> Self {
        Self {
            strategy: RenderStrategy::default(),
            revalidate: None,
            has_static_params: false,
            static_paths: Vec::new(),
            has_dynamic_slots: false,
            hydration: HydrationMode::Load,
            server_components: false,
            force_dynamic: false,
        }
    }
}

/// Publish the route manifest at `output_file`.
///
/// Through [`ruvyxa_bundler::atomic_file::write_atomic`], not `fs::write`.
/// `fs::write` truncates and then writes, so for the length of the write the
/// document on disk is empty or partial: a zero-byte `routes.json` that an
/// interrupted build leaves behind still looks present to anything that only
/// tests for existence, and `ruvyxa start` reading a directory that is being
/// rebuilt sees a parse error for a manifest nobody wrote.
///
/// The shared helper is the one this crate wants rather than a local
/// temp-and-rename, because its module documents the three ways a hand-rolled
/// copy of those steps has already drifted here — a temporary named after the
/// target alone, a temporary left behind when the first write fails, and a
/// failed rename answered by writing over the target.
pub fn write_manifest(manifest: &RouteManifest, output_file: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|error| RuvyxaError::Message(error.to_string()))?;
    ruvyxa_bundler::atomic_file::write_atomic(output_file, json.as_bytes())?;
    Ok(())
}

/// One intercepting route reachable from a particular route.
///
/// `app/feed/@modal/(.)photo/page.tsx` declares that a soft navigation to
/// `/feed/photo` should render `page.tsx` into the `modal` slot of the layout
/// at `app/feed`, leaving whatever is already on screen mounted underneath. A
/// hard load of `/feed/photo` is unaffected: it renders `app/feed/photo`, the
/// ordinary route, which must exist for the interception to be accepted at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteIntercept {
    /// Directory holding the `@name` folder, as a route id (`app/feed`).
    pub level: String,
    /// Slot name without the `@`, which is the prop the layout receives.
    pub name: String,
    /// Route pattern this interception covers, in the same shape as
    /// [`RouteEntry::path`] so one matcher answers both.
    pub target: String,
    /// The marker the author wrote, kept for diagnostics.
    pub marker: String,
    /// File that renders the interception.
    pub file: PathBuf,
}

/// One parallel-route slot resolved for a particular route.
///
/// A `@name` directory beside a `layout.tsx` declares a slot that layout
/// receives as a prop. The slot matches the URL independently of the page: for
/// `/dashboard/reports`, the slot at `app/dashboard/@team` renders
/// `@team/reports/page.tsx` if it exists, and `@team/default.tsx` otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSlot {
    /// Directory holding the `@name` folder, as a route id (`app/dashboard`).
    /// This is the level whose layout receives the slot.
    pub level: String,
    /// Slot name without the `@`, which is the prop name the layout sees.
    pub name: String,
    /// File that renders this slot for this route.
    pub file: PathBuf,
}
