# Shared search sorting

## 1. Scope / trigger

Applies to Core discovery and every shell exposing search. Core owns supported order/category combinations, request parameters, response parsing and pagination; shells own input and presentation.

## 2. Signatures

`SearchQuery { keyword: String, category: SearchCategory, page: u32, sort: SearchSort, filter: SearchFilter }`; `SearchQuery::validate() -> Result<()>`; `SearchSort::available_for(SearchCategory) -> &'static [SearchSort]`; `Core::search(SearchQuery) -> Result<CandidatePage>`. These types are exported from the existing Rust root domain boundary. The optional video-channel filter and separate PGC genre browsing are specified in [browse/download contracts](browse-download-ux.md).

CLI: `bili search QUERY --type all|video|bangumi|film --page N --sort relevance|views|newest|danmaku|favorites|comments [--auth MODE]`. Omitted sort defaults to relevance. Missing serialized SearchQuery.sort also defaults to Relevance; Rust source callers must supply the new field and matched consumers must rebuild.

## 3. Contracts

- Relevance preserves the existing request with no order parameter, including All's per-category fan-out. Video alone supports non-default sorting: Views -> click, Newest -> pubdate, Danmaku -> dm, Favorites -> stow, Comments -> scores. Do not invent non-default series/movie or mixed-category ordering.
- Validate nonempty keyword (maximum 256 UTF-8 bytes), page 1..=1000 and the supported category/sort combination before signing/network. CLI calls the same Core validation before authentication so an invalid combination cannot trigger QR, cookie input or stored-account loading.
- Keep the existing API path, signing, allowed hosts, connection budget and typed failure handling. The private search-specific fetch seam permits loopback tests; it is not an alternate production endpoint or public transport configuration.
- Preserve source order within each category, page numbers, totals, has_more, category grouping and existing reference deduplication. Never locally sort a page or claim a merged global order across categories. The CLI search/error envelope stays the same; optional card/page metadata is additive and unknown values remain unknown.
- GUI derives choices from Core support. Changing sort submits the current keyword/category at page 1; category changes visibly restrict selection to supported orders. Requested/submitted query state includes sort; pagination retains the submitted query, and existing return/stale-result behavior remains intact. This does not authorize unrelated navigation or styling changes.

## 4. Validation & error matrix

| Condition | Result |
| --- | --- |
| Empty/oversized keyword or page outside 1..=1000 | InvalidInput before fetch |
| Non-default sort with All/Bangumi/Film | InvalidInput before auth/signing/fetch; CLI exit 2 with typed JSON error when requested |
| Unknown CLI sort value | Clap usage error, exit 2 |
| Verification/access failure, malformed response | Preserve existing typed error; never return empty success or silently change order |
| Default sort for any existing category | Existing default requests and paging semantics |

## 5. Good / base / bad cases

Good: video/views page 2 requests order=click and retains the server's second-page sequence. Base: All/relevance keeps the three existing default category pages. Bad: sorting a downloaded page locally, applying video order to Film, or prompting for login before rejecting an invalid combination.

## 6. Tests required

Owning Core discovery tests use deterministic local HTTP to assert outgoing category/order/page parameters and distinct two-page results, all mappings, default omission, typed failures and pre-fetch rejection. CLI tests exercise real parsing and typed pre-auth errors with isolated profiles and update checks disabled. GUI tests exercise page reset, category support and submitted-order pagination; do not use source-text assertions. Preserve default and credential-safety regressions.

Offline tests establish application behavior, not current upstream support. Live anonymous search acceptance requires explicit separate authorization. Test existence is not passing evidence; per-session results remain in task records and ignored logs.

## 7. Wrong versus correct

Wrong: CLI sorts CandidatePage.items by a displayed metric and GUI defines a separate sort support table. Correct: both forward Core SearchSort, Core validates support and requests server order, and the original returned page sequence reaches presentation.
