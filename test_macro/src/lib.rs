use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

#[proc_macro_attribute]
pub fn test(
    _attr: TokenStream,
    item: TokenStream,
) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);

    let name = &function.sig.ident;

    let name_string = name.to_string();

    let registration_name = syn::Ident::new(
        &format!("__OXENNA_TEST_{}", name),
        name.span(),
    );

    let expanded = quote! {
        #function

        #[used]
        #[unsafe(link_section = ".kernel_tests")]
        static #registration_name: crate::test::Test =
            crate::test::Test {
                name: #name_string,
                function: #name,
            };
    };

    TokenStream::from(expanded)
}
