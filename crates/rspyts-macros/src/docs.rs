//! Doc-comment extraction.
//!
//! Doc comments on bridged items propagate into the manifest and from
//! there to Python docstrings, TypeScript doc comments, and JSON Schema
//! descriptions (type-system §3).

/// Collect the `#[doc = "…"]` lines of `attrs` into one string.
///
/// Lines are joined with `\n`; the single leading space rustc inserts for
/// `/// text` is stripped from each line. Returns an empty string when the
/// item has no docs.
pub fn extract_docs(attrs: &[syn::Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text),
                ..
            }) = &nv.value
            {
                let value = text.value();
                lines.push(value.strip_prefix(' ').unwrap_or(&value).to_string());
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn joins_lines_and_strips_one_leading_space() {
        let item: syn::ItemStruct = parse_quote! {
            /// First line.
            /// Second line.
            struct S;
        };
        assert_eq!(extract_docs(&item.attrs), "First line.\nSecond line.");
    }

    #[test]
    fn preserves_extra_indentation_beyond_the_first_space() {
        let item: syn::ItemStruct = parse_quote! {
            /// Header:
            ///   - indented bullet
            struct S;
        };
        assert_eq!(extract_docs(&item.attrs), "Header:\n  - indented bullet");
    }

    #[test]
    fn empty_when_undocumented_and_ignores_other_attrs() {
        let item: syn::ItemStruct = parse_quote! {
            #[derive(Debug)]
            struct S;
        };
        assert_eq!(extract_docs(&item.attrs), "");
    }

    #[test]
    fn keeps_blank_doc_lines() {
        let item: syn::ItemStruct = parse_quote! {
            /// Paragraph one.
            ///
            /// Paragraph two.
            struct S;
        };
        assert_eq!(
            extract_docs(&item.attrs),
            "Paragraph one.\n\nParagraph two."
        );
    }
}
