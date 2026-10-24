//! Links in documentation comments.
//!
//! rust-analyzer's `doc_links` turns `[Foo]` in a doc comment into a
//! clickable link and resolves `#[doc(html_root_url)]` to a docs.rs page.
//! This module held three functions and none of them could have done
//! that here:
//!
//! - `rewrite_links` returned its input, under "for now, pass through —
//!   link resolution will be enhanced when the workspace symbol index is
//!   integrated". The index has been integrated for months.
//! - `external_docs` returned `Default::default()`, and its own comment
//!   says why it always will: Sail has no docs.rs.
//! - `extract_doc_links` scanned the raw text for `/**` and `//!` — those
//!   are *Rust's* doc comments. Sail's are `///` and `/*!`
//!   (`lexer.mll:310`, `:342`), so it found nothing in any Sail file
//!   ever written. It carried the comment "Comments are not emitted as
//!   tokens (they're trivia), so we scan the raw text", which is false
//!   here by deliberate choice: `DOC_COMMENT` is non-trivia so the item
//!   tree can read documentation off it, and hover does.
//!
//! Deleted rather than fixed. Making it work is not repair, it is a new
//! feature — a doc-comment reference has no target until something
//! resolves it against the symbol index, and `document_links_for_file`,
//! which answers `textDocument/documentLink`, is given a file and a URL
//! and no index. If that feature is wanted, the shape is: `Analysis`
//! holds the index, so the resolution belongs beside
//! `document_links_inner`, and the scan asks the CST for `DOC_COMMENT`
//! tokens rather than searching text for a delimiter.
