use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{braced, bracketed, Attribute, Expr, FnArg, Ident, ItemFn, Pat, Token, parse_macro_input};

/// A parameter specification that supports single, multi-value, or optional syntax.
/// - `smp = 4` → single value
/// - `smp = {1, 2, 4}` → multiple values (cartesian product)
/// - `cpu = [CpuModel::Qemu64, CpuModel::Host]` → optional parameter with implicit None variant
struct ParamSpec {
    name: Ident,
    values: Vec<Expr>,
    optional: bool,
}

impl Parse for ParamSpec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let (values, optional) = if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            let vals = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
            (vals, false)
        } else if input.peek(syn::token::Bracket) {
            let content;
            bracketed!(content in input);
            let vals = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();
            (vals, true)
        } else {
            (vec![input.parse::<Expr>()?], false)
        };
        Ok(ParamSpec {
            name,
            values,
            optional,
        })
    }
}

/// Registers a test function with optional parameterization.
///
/// Supports cartesian product expansion, optional parameters, and skip:
/// ```ignore
/// #[test_fn(machine = {Machine::Pc, Machine::Q35}, smp = {1, 2, 4})]
/// fn test_kernel_boot(machine: Machine, smp: u8) -> Result<()> { ... }
///
/// #[test_fn(machine = {Machine::Pc, Machine::Q35}, cpu = [CpuModel::Qemu64, CpuModel::Host])]
/// fn test_cpu(machine: Machine, cpu: Option<CpuModel>) -> Result<()> { ... }
///
/// #[test_fn(skip = "requires tap networking")]
/// fn test_tap_migration() -> Result<()> { ... }
/// ```
///
/// Optional parameters (using `[]` syntax) add an implicit `None` variant to the
/// cartesian product. The function parameter type must be `Option<T>`. The label
/// omits optional parameters when their value is `None`.
///
/// Generates one `TestEntry` per combination, auto-registered via `linkme`.
#[proc_macro_attribute]
pub fn test_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    let (own_specs, skip_reason) = extract_skip(parse_specs(attr));

    // Collect specs from remaining stacked #[test_fn(...)] attributes
    let mut all_spec_sets = vec![own_specs];
    let mut other_attrs = Vec::new();
    let mut skip = skip_reason;

    for a in &input.attrs {
        if a.path().is_ident("test_fn") {
            let (specs, sr) = extract_skip(parse_specs_from_attr(a));
            all_spec_sets.push(specs);
            if sr.is_some() {
                skip = sr;
            }
        } else {
            other_attrs.push(a.clone());
        }
    }

    let skip_token = match &skip {
        Some(reason) => quote! { Some(#reason) },
        None => quote! { None },
    };

    let name = &input.sig.ident;
    let name_str = name.to_string();
    let block = &input.block;
    let vis = &input.vis;
    let ret = &input.sig.output;
    let params = &input.sig.inputs;

    // Expand each annotation's specs into resolved combinations via cartesian product
    let mut all_combos: Vec<Vec<(Ident, Option<Expr>, bool)>> = Vec::new();
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
        let props_fn = format_ident!("{}_props", name);
        let expanded = quote! {
            #(#other_attrs)*
            #vis fn #name() #ret {
                (|| #block)()
            }

            fn #props_fn() -> Vec<(&'static str, String)> {
                Vec::new()
            }

            #[linkme::distributed_slice(crate::TESTS)]
            static #static_name: crate::TestEntry = crate::TestEntry {
                name: #name_str,
                props_fn: #props_fn,
                test_fn: #name,
                skip: #skip_token,
            };
        };
        return expanded.into();
    }

    // Parameterized: generate numbered functions, each auto-registered
    let mut fn_defs = Vec::new();

    for (i, combo) in all_combos.iter().enumerate() {
        let fn_name = format_ident!("{}_{}", name, i);
        let props_fn = format_ident!("{}_{}_props", name, i);
        let static_name = format_ident!("_{}_{}", name_str.to_uppercase(), i);

        let bindings = make_bindings(params, combo);
        let props_code = make_props_code(combo);

        fn_defs.push(quote! {
            #(#other_attrs)*
            #[allow(unused_variables)]
            #vis fn #fn_name() #ret {
                #(#bindings)*
                (|| #block)()
            }

            #[allow(unused_variables)]
            fn #props_fn() -> Vec<(&'static str, String)> {
                #(#bindings)*
                #props_code
            }

            #[linkme::distributed_slice(crate::TESTS)]
            static #static_name: crate::TestEntry = crate::TestEntry {
                name: #name_str,
                props_fn: #props_fn,
                test_fn: #fn_name,
                skip: #skip_token,
            };
        });
    }

    let expanded = quote! {
        #(#fn_defs)*
    };

    expanded.into()
}

/// Extracts a `skip = "reason"` spec from the list, returning remaining specs
/// and the optional skip reason string.
fn extract_skip(specs: Vec<ParamSpec>) -> (Vec<ParamSpec>, Option<String>) {
    let mut remaining = Vec::new();
    let mut skip_reason = None;
    for spec in specs {
        if spec.name == "skip" {
            if let Some(Expr::Lit(lit)) = spec.values.first() {
                if let syn::Lit::Str(s) = &lit.lit {
                    skip_reason = Some(s.value());
                } else {
                    panic!("skip value must be a string literal");
                }
            } else {
                panic!("skip value must be a string literal");
            }
        } else {
            remaining.push(spec);
        }
    }
    (remaining, skip_reason)
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
/// Optional params include an implicit `None` variant.
fn cartesian_product(specs: &[ParamSpec]) -> Vec<Vec<(Ident, Option<Expr>, bool)>> {
    let mut result: Vec<Vec<(Ident, Option<Expr>, bool)>> = vec![vec![]];
    for spec in specs {
        let mut new_result = Vec::new();
        for combo in &result {
            if spec.optional {
                // Add the None variant
                let mut none_combo = combo.clone();
                none_combo.push((spec.name.clone(), None, true));
                new_result.push(none_combo);
            }
            for value in &spec.values {
                let mut new_combo = combo.clone();
                new_combo.push((spec.name.clone(), Some(value.clone()), spec.optional));
                new_result.push(new_combo);
            }
        }
        result = new_result;
    }
    result
}

fn make_bindings(
    params: &Punctuated<FnArg, Token![,]>,
    combo: &[(Ident, Option<Expr>, bool)],
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

            let (_, value, optional) = combo
                .iter()
                .find(|(name, _, _)| name == param_name)
                .unwrap_or_else(|| {
                    panic!("missing attribute value for parameter `{param_name}`")
                });

            if *optional {
                match value {
                    Some(expr) => quote! { let #param_name: #param_type = Some(#expr); },
                    None => quote! { let #param_name: #param_type = None; },
                }
            } else {
                let expr = value.as_ref().unwrap();
                quote! { let #param_name: #param_type = #expr; }
            }
        })
        .collect()
}

fn make_props_code(combo: &[(Ident, Option<Expr>, bool)]) -> proc_macro2::TokenStream {
    // Only include params that are not optional-None
    let visible: Vec<&(Ident, Option<Expr>, bool)> = combo
        .iter()
        .filter(|(_, value, optional)| !(*optional && value.is_none()))
        .collect();

    if visible.is_empty() {
        quote! { Vec::new() }
    } else {
        let entries: Vec<proc_macro2::TokenStream> = visible
            .iter()
            .map(|(name, _, optional)| {
                let key = name.to_string();
                if *optional {
                    quote! { (#key, format!("{}", #name.unwrap())) }
                } else {
                    quote! { (#key, format!("{}", #name)) }
                }
            })
            .collect();

        quote! { vec![#(#entries),*] }
    }
}
