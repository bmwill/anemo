use proc_macro2::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream};
use quote::TokenStreamExt;

pub mod manual;

/// Service code generation for client
pub mod client;
/// Service code generation for Server
pub mod server;

// Generate a singular line of a doc comment
fn generate_doc_comment<S: AsRef<str>>(comment: S) -> TokenStream {
    let mut doc_stream = TokenStream::new();

    doc_stream.append(Ident::new("doc", Span::call_site()));
    doc_stream.append(Punct::new('=', Spacing::Alone));
    doc_stream.append(Literal::string(comment.as_ref()));

    let group = Group::new(Delimiter::Bracket, doc_stream);

    let mut stream = TokenStream::new();
    stream.append(Punct::new('#', Spacing::Alone));
    stream.append(group);
    stream
}

// Generate a larger doc comment composed of many lines of doc comments
fn generate_doc_comments<T: AsRef<str>>(comments: &[T]) -> TokenStream {
    let mut stream = TokenStream::new();

    for comment in comments {
        stream.extend(generate_doc_comment(comment));
    }

    stream
}

fn naive_snake_case(name: &str) -> String {
    let mut s = String::new();
    let mut it = name.chars().peekable();

    while let Some(x) = it.next() {
        s.push(x.to_ascii_lowercase());
        if let Some(y) = it.peek() {
            if y.is_uppercase() {
                s.push('_');
            }
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snake_case() {
        for case in &[
            ("Service", "service"),
            ("ThatHasALongName", "that_has_a_long_name"),
            ("greeter", "greeter"),
            ("ABCServiceX", "a_b_c_service_x"),
        ] {
            assert_eq!(naive_snake_case(case.0), case.1)
        }
    }
}
