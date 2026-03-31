use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{braced, Attribute, Expr, FnArg, Ident, ItemFn, Pat, Token, parse_macro_input};

/// A parameter specification that supports single or multi-value syntax.
/// - `smp = 4` → single value
/// - `smp = {1, 2, 4}` → multiple values (cartesian product)
struct ParamSpec {
    name: Ident,
    values: Vec<Expr>,
}

impl Parse for ParamSpec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let values = if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            Punctuated::<Expr, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect()
        } else {
            vec![input.parse::<Expr>()?]
        };
        Ok(ParamSpec { name, values })
    }
}

/// Registers a test function with optional parameterization.
///
/// Supports cartesian product expansion:
/// ```ignore
/// #[test_fn(machine = {Machine::Pc, Machine::Q35}, smp = {1, 2, 4})]
/// fn test_kernel_boot(machine: Machine, smp: u8) -> Result<()> { ... }
/// ```
/// Generates one `TestEntry` per combination, auto-registered via `linkme`.
#[proc_macro_attribute]
pub fn test_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    let own_specs = parse_specs(attr);

    // Collect specs from remaining stacked #[test_fn(...)] attributes
    let mut all_spec_sets = vec![own_specs];
    let mut other_attrs = Vec::new();

    for a in &input.attrs {
        if a.path().is_ident("test_fn") {
            all_spec_sets.push(parse_specs_from_attr(a));
        } else {
            other_attrs.push(a.clone());
        }
    }

    let name = &input.sig.ident;
    let name_str = name.to_string();
    let block = &input.block;
    let vis = &input.vis;
    let ret = &input.sig.output;
    let params = &input.sig.inputs;

    // Expand each annotation's specs into resolved combinations via cartesian product
    let mut all_combos: Vec<Vec<(Ident, Expr)>> = Vec::new();
    for specs in &all_spec_sets {
        if specs.is_empty() {
            all_combos.push(Vec::new());
        } else {
            all_combos.extend(cartesian_product(specs));
        }
    }

    if all_combos.len() == 1 && all_combos[0].is_empty() {
        // Non-parameterized: single function, auto-registered
        let static_name = format_ident!("_{}", name_str.to_uppercase());
        let label_fn = format_ident!("{}_label", name);
        let expanded = quote! {
            #(#other_attrs)*
            #vis fn #name() #ret {
                (|| #block)()
            }

            fn #label_fn() -> String {
                #name_str.to_string()
            }

            #[linkme::distributed_slice(crate::TESTS)]
            static #static_name: crate::TestEntry = (#label_fn, #name);
        };
        return expanded.into();
    }

    // Parameterized: generate numbered functions, each auto-registered
    let mut fn_defs = Vec::new();

    for (i, combo) in all_combos.iter().enumerate() {
        let fn_name = format_ident!("{}_{}", name, i);
        let label_fn = format_ident!("{}_{}_label", name, i);
        let static_name = format_ident!("_{}_{}", name_str.to_uppercase(), i);

        let bindings = make_bindings(params, combo);
        let label_code = make_label_code(&name_str, combo);

        fn_defs.push(quote! {
            #(#other_attrs)*
            #vis fn #fn_name() #ret {
                #(#bindings)*
                (|| #block)()
            }

            fn #label_fn() -> String {
                #(#bindings)*
                #label_code
                __test_label
            }

            #[linkme::distributed_slice(crate::TESTS)]
            static #static_name: crate::TestEntry = (#label_fn, #fn_name);
        });
    }

    let expanded = quote! {
        #(#fn_defs)*
    };

    expanded.into()
}

fn parse_specs(attr: TokenStream) -> Vec<ParamSpec> {
    if attr.is_empty() {
        return Vec::new();
    }
    let parser = Punctuated::<ParamSpec, Token![,]>::parse_terminated;
    parser
        .parse(attr)
        .expect("failed to parse test_fn attributes")
        .into_iter()
        .collect()
}

fn parse_specs_from_attr(attr: &Attribute) -> Vec<ParamSpec> {
    let tokens = match &attr.meta {
        syn::Meta::List(list) => list.tokens.clone(),
        _ => return Vec::new(),
    };
    let parser = Punctuated::<ParamSpec, Token![,]>::parse_terminated;
    parser
        .parse2(tokens)
        .expect("failed to parse test_fn attributes")
        .into_iter()
        .collect()
}

/// Computes the cartesian product of all parameter value sets.
fn cartesian_product(specs: &[ParamSpec]) -> Vec<Vec<(Ident, Expr)>> {
    let mut result: Vec<Vec<(Ident, Expr)>> = vec![vec![]];
    for spec in specs {
        let mut new_result = Vec::new();
        for combo in &result {
            for value in &spec.values {
                let mut new_combo = combo.clone();
                new_combo.push((spec.name.clone(), value.clone()));
                new_result.push(new_combo);
            }
        }
        result = new_result;
    }
    result
}

fn make_bindings(
    params: &Punctuated<FnArg, Token![,]>,
    combo: &[(Ident, Expr)],
) -> Vec<proc_macro2::TokenStream> {
    params
        .iter()
        .map(|arg| {
            let FnArg::Typed(pat_type) = arg else {
                panic!("test_fn does not support self parameters");
            };
            let Pat::Ident(pat_ident) = pat_type.pat.as_ref() else {
                panic!("test_fn requires simple parameter names");
            };
            let param_name = &pat_ident.ident;
            let param_type = &pat_type.ty;

            let (_, value) = combo
                .iter()
                .find(|(name, _)| name == param_name)
                .unwrap_or_else(|| {
                    panic!("missing attribute value for parameter `{param_name}`")
                });

            quote! { let #param_name: #param_type = #value; }
        })
        .collect()
}

fn make_label_code(
    name_str: &str,
    combo: &[(Ident, Expr)],
) -> proc_macro2::TokenStream {
    if combo.is_empty() {
        quote! { let __test_label = #name_str.to_string(); }
    } else {
        let keys: Vec<String> = combo.iter().map(|(name, _)| name.to_string()).collect();
        let idents: Vec<&Ident> = combo.iter().map(|(name, _)| name).collect();
        let fmt_parts: Vec<_> = keys.iter().map(|k| format!("{k}={{}}")).collect();
        let fmt_str = format!("{}({})", name_str, fmt_parts.join(", "));
        quote! { let __test_label = format!(#fmt_str, #(#idents),*); }
    }
}
