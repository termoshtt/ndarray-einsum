//! proc-macro based einsum implementation

use einsum_solver::subscripts::{Label, Subscripts};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use proc_macro_error::{abort_call_site, proc_macro_error, OptionExt, ResultExt};
use quote::quote;
use std::{
    collections::{hash_map::Entry, HashMap},
    str::FromStr,
};
use syn::parse::Parser;

/// proc-macro based einsum
///
/// ```
/// let a = ndarray::array![[1.0, 2.0], [3.0, 4.0]];
/// let b = ndarray::array![[1.0, 2.0], [3.0, 4.0]];
/// let c = einsum_derive::einsum!("ij,jk->ik", a, b);
/// ```
///
/// Number of input mismatch causes compile error:
///
/// ```compile_fail
/// let a = ndarray::array![[1.0, 2.0], [3.0, 4.0]];
/// let c = einsum_derive::einsum!("ij,jk->ik", a);
/// ```
#[proc_macro_error]
#[proc_macro]
pub fn einsum(input: TokenStream) -> TokenStream {
    einsum2(input.into()).into()
}

fn einsum2(input: TokenStream2) -> TokenStream2 {
    let (subscripts, args) = parse_einsum_args(input);

    // Validate subscripts
    let subscripts = Subscripts::from_str(&subscripts)
        .ok()
        .expect_or_abort("Invalid subscripts");
    if subscripts.inputs.len() != args.len() {
        abort_call_site!("Argument number mismatch");
    }

    // Generate pre-requirement parts:
    //
    // - Define variable for input tensor expression
    //   ```
    //   let arg0 = a;
    //   ```
    //
    // - Define size of dimensions for each input tensors
    //   ```
    //   let (n_0_i, n_0_j) = arg0.dim();
    //   ```
    //
    // - Set global size if not defined
    //   ```
    //   let n_i = n_0_i;
    //   ```
    //
    // - Or insert runtime check of sizes
    //   ```
    //   assert_eq!(n_i, n_1_i);
    //   ```
    //
    let mut n_idents: HashMap<char, proc_macro2::Ident> = HashMap::new();
    let mut pre_requirements_tt = Vec::new();
    for argc in 0..args.len() {
        let name = quote::format_ident!("arg{}", argc);
        let arg = &args[argc];
        let mut n_index_each = Vec::new();
        let mut def_or_assert = Vec::new();
        for label in &subscripts.inputs[argc] {
            match label {
                Label::Index(i) => {
                    let n = quote::format_ident!("n_{}_{}", argc, i);
                    match n_idents.entry(*i) {
                        Entry::Occupied(entry) => {
                            let n_ = entry.get();
                            def_or_assert.push(quote! {
                                assert_eq!(#n_, #n);
                            });
                        }
                        Entry::Vacant(entry) => {
                            let n_ident = quote::format_ident!("n_{}", i);
                            def_or_assert.push(quote! {
                                let #n_ident = #n;
                            });
                            entry.insert(n_ident);
                        }
                    }
                    n_index_each.push(n);
                }
                _ => unimplemented!(),
            }
        }
        pre_requirements_tt.push(quote! {
            let #name = #arg;
            let (#(#n_index_each),*) = #name.dim();
            #( #def_or_assert )*
        });
    }

    // Define output array
    let output_ident = quote::format_ident!("out");
    let mut n_output = Vec::new();
    for label in &subscripts.output {
        match label {
            Label::Index(i) => n_output.push(n_idents.get(i).expect_or_abort("")),
            _ => unimplemented!(),
        }
    }
    let output_tt = quote! {
        let #output_ident = ndarray::Array::<f64, _>::zeros((#(#n_output),*));
    };

    // TODO: Compute contraction
    let contraction_tt = quote! {};

    quote! {
        {
            #(#pre_requirements_tt)*
            #output_tt
            #contraction_tt
            #output_ident
        }
    }
}

fn parse_einsum_args(input: TokenStream2) -> (String, Vec<syn::Expr>) {
    let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
    let args = parser
        .parse2(input)
        .expect_or_abort("Invalid input for einsum!");
    let mut iter = args.into_iter();
    let subscripts = if let Some(syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(lit),
        attrs: _,
    })) = iter.next()
    {
        lit.value()
    } else {
        abort_call_site!("einsum! must start with subscript string literal")
    };
    let args = iter.collect::<Vec<_>>();
    (subscripts, args)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    #[test]
    fn test_snapshots() {
        let input = TokenStream2::from_str(r#""ij,jk->ik", a, b"#).unwrap();
        let tt = format_block(einsum2(input).to_string());
        insta::assert_snapshot!(tt, @r###"
            {
                let arg0 = a;
                let (n_0_i, n_0_j) = arg0.dim();
                let n_i = n_0_i;
                let n_j = n_0_j;
                let arg1 = b;
                let (n_1_j, n_1_k) = arg1.dim();
                assert_eq!(n_j, n_1_j);
                let n_k = n_1_k;
                let out = ndarray::Array::<f64, _>::zeros((n_i, n_k));
                out
            }
        "###);
    }

    /// Format generated Rust code using `rustfmt` run as external process.
    pub fn format_block(tt: String) -> String {
        let tt = format!("fn main() {{ {} }}", tt);

        let mut child = Command::new("rustfmt")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn rustfmt process");

        // Write input from another thread for avoiding deadlock.
        // See https://doc.rust-lang.org/std/process/index.html#handling-io
        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        std::thread::spawn(move || {
            stdin
                .write_all(tt.as_bytes())
                .expect("Failed to write to stdin");
        });
        let output = child
            .wait_with_output()
            .expect("Failed to wait output of rustfmt process");

        // non-UTF8 comment should be handled in the tokenize phase,
        // and not be included in IR.
        let out = String::from_utf8(output.stdout).expect("rustfmt output contains non-UTF8 input");

        out.strip_prefix("fn main() {\n")
            .and_then(|out| out.strip_suffix("}\n"))
            .unwrap()
            .to_string()
    }
}
