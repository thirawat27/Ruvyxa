use crate::manifest::RenderStrategy;

/// Cache-control for a pre-rendered document.
///
/// Safe to store, never safe to pin: a redeploy replaces the document under the
/// same URL, and a reader holding a heuristically-cached copy would keep seeing
/// the old site with no way to know.
pub const DOCUMENT_CACHE_CONTROL: &str = "public, max-age=0, must-revalidate";

/// The revalidation window an ISR route that named none is given.
pub const DEFAULT_REVALIDATE_SECONDS: u64 = 60;

/// How long a stale ISR document may still be served while it refreshes.
///
/// The stale window is `ISR_EXPIRE_SECONDS - revalidate`, which is the formula
/// Next.js ships in production (its `expireTime`, one year by default). The
/// directive has to carry a number: RFC 5861 defines
/// `stale-while-revalidate=<delta-seconds>`, and Netlify's CDN documents only
/// the numeric form — a bare directive is dropped there, which silently turns
/// every refresh into a blocking render.
pub const ISR_EXPIRE_SECONDS: u64 = 31_536_000;

/// What a server sends with a document it just rendered, by strategy.
///
/// ISR advertises the project's own clock so a CDN in front of the server can
/// hold the page for exactly as long as the project asked, and refresh it
/// without a gap. A per-request render advertises nothing cacheable: it may
/// carry one visitor's data, and a shared cache with no instruction has been
/// observed to store it anyway under heuristic freshness.
///
/// `max-age=0` is the same guard [`DOCUMENT_CACHE_CONTROL`] carries and for the
/// same reason: `s-maxage` speaks to the shared cache only, so an ISR response
/// that named no `max-age` left the *browser* with no freshness instruction and
/// heuristic caching applies.
///
/// This lives here rather than beside the deploy manifest because both request
/// hosts need the same answer: `ruvyxa start` serves through Axum and every
/// deployed build serves through `createHandler`, and the Axum side sent no
/// cache-control at all for a page it had just rendered. `documentCacheControl`
/// in `@ruvyxa/core` and in `packages/ruvyxa/runtime/serverless-handler.mjs` are
/// the JavaScript halves; all of them are replayed against
/// `tests/fixtures/deploy-output-conformance.json`.
pub fn document_cache_control(strategy: RenderStrategy, revalidate: Option<u64>) -> String {
    match strategy {
        RenderStrategy::Isr => {
            let revalidate = revalidate.unwrap_or(DEFAULT_REVALIDATE_SECONDS);
            format!(
                "public, max-age=0, s-maxage={revalidate}, stale-while-revalidate={}",
                ISR_EXPIRE_SECONDS.saturating_sub(revalidate)
            )
        }
        RenderStrategy::Ssg | RenderStrategy::Csr => DOCUMENT_CACHE_CONTROL.to_string(),
        RenderStrategy::Ssr | RenderStrategy::Ppr => "no-store".to_string(),
    }
}

/// The strategies whose document is stored bytes, and may therefore be validated.
///
/// [`DOCUMENT_CACHE_CONTROL`] tells a browser to revalidate before every reuse,
/// and without a validator that revalidation can only be answered with the whole
/// document again — so a page a reader already holds was re-sent in full on
/// every navigation, on both hosts. ISR is the same question with `s-maxage` in
/// front of it.
///
/// `Ssr` and `Ppr` are absent because their document is produced for this
/// request: it may carry one visitor's data, it may still be streaming, and it
/// is `no-store` either way, so there is nothing for a validator to be about.
///
/// `DOCUMENT_VALIDATOR_STRATEGIES` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs` is the JavaScript half; both
/// are replayed against `tests/fixtures/deploy-output-conformance.json`. What
/// the two deliberately do **not** share is the validator's value — this host
/// hashes with blake3 and the deployed one with SHA-256 — because a validator is
/// opaque and scoped to the origin that issued it, and no client ever holds one
/// from both.
pub const DOCUMENT_VALIDATOR_STRATEGIES: [RenderStrategy; 3] = [
    RenderStrategy::Ssg,
    RenderStrategy::Csr,
    RenderStrategy::Isr,
];

/// Whether a document served under `strategy` carries a validator.
pub fn document_has_validator(strategy: RenderStrategy) -> bool {
    DOCUMENT_VALIDATOR_STRATEGIES.contains(&strategy)
}
