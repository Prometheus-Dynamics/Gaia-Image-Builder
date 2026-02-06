use proc_macro::TokenStream;

use quote::quote;
use syn::parse_quote;
use syn::{
    Attribute, Expr, ExprArray, ExprLit, ExprPath, ItemStruct, Lit, Meta, Token, parse::Parser,
    spanned::Spanned,
};

#[proc_macro_attribute]
#[allow(non_snake_case)]
pub fn Task(attr: TokenStream, item: TokenStream) -> TokenStream {
    match task_impl(attr, item) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
#[allow(non_snake_case)]
pub fn Module(attr: TokenStream, item: TokenStream) -> TokenStream {
    match module_impl(attr, item) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error().into(),
    }
}

fn lit_str(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.value()),
        _ => Err(syn::Error::new(expr.span(), "expected string literal")),
    }
}

fn lit_bool(expr: &Expr) -> syn::Result<bool> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Bool(b), ..
        }) => Ok(b.value),
        _ => Err(syn::Error::new(expr.span(), "expected bool literal")),
    }
}

fn expr_array_strings(expr: &Expr) -> syn::Result<Vec<String>> {
    let Expr::Array(ExprArray { elems, .. }) = expr else {
        return Err(syn::Error::new(expr.span(), "expected array literal"));
    };
    let mut out = Vec::new();
    for e in elems {
        out.push(lit_str(e)?);
    }
    Ok(out)
}

fn expr_array_paths(expr: &Expr) -> syn::Result<Vec<syn::Path>> {
    let Expr::Array(ExprArray { elems, .. }) = expr else {
        return Err(syn::Error::new(expr.span(), "expected array literal"));
    };
    let mut out = Vec::new();
    for e in elems {
        match e {
            Expr::Path(ExprPath { path, .. }) => out.push(path.clone()),
            _ => return Err(syn::Error::new(e.span(), "expected path (identifier)")),
        }
    }
    Ok(out)
}

fn expr_type(expr: &Expr) -> syn::Result<syn::Type> {
    match expr {
        Expr::Path(ExprPath { path, .. }) => Ok(syn::Type::Path(syn::TypePath {
            qself: None,
            path: path.clone(),
        })),
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => syn::parse_str::<syn::Type>(&s.value()).map_err(|e| syn::Error::new(expr.span(), e)),
        _ => Err(syn::Error::new(
            expr.span(),
            "expected type (path) or string",
        )),
    }
}

fn drop_our_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
    attrs
        .iter()
        .filter(|a| {
            let Meta::Path(p) = &a.meta else {
                return true;
            };
            let Some(ident) = p.get_ident() else {
                return true;
            };
            ident != "Task" && ident != "Module"
        })
        .cloned()
        .collect()
}

struct TaskMeta {
    id: String,
    module: String,
    phase: String,
    config_path: String,
    provides: Vec<String>,
    after: Vec<String>,
    after_if: Vec<(String, String)>,
    default_label: String,
    core: bool,
}

fn task_impl(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut st: ItemStruct = syn::parse(item)?;
    st.attrs = drop_our_attrs(&st.attrs);
    let struct_ident = st.ident.clone();

    let parser = syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse(attr)?;

    let mut id: Option<String> = None;
    let mut module: Option<String> = None;
    let mut phase: Option<String> = None;
    let mut config_ty: Option<syn::Type> = None;
    let mut config_path: Option<String> = None;
    let mut provides: Vec<String> = Vec::new();
    let mut after: Vec<String> = Vec::new();
    let mut after_if: Vec<(String, String)> = Vec::new();
    let mut default_label: Option<String> = None;
    let mut core: bool = false;

    for m in metas {
        let Meta::NameValue(nv) = m else {
            return Err(syn::Error::new(m.span(), "expected key = value"));
        };
        let Some(key) = nv.path.get_ident().map(|i| i.to_string()) else {
            return Err(syn::Error::new(nv.path.span(), "expected ident key"));
        };
        let v = &nv.value;
        match key.as_str() {
            "id" => id = Some(lit_str(v)?),
            "module" => module = Some(lit_str(v)?),
            "phase" => phase = Some(lit_str(v)?),
            "config" => config_ty = Some(expr_type(v)?),
            "config_path" => config_path = Some(lit_str(v)?),
            "provides" => provides = expr_array_strings(v)?,
            "after" => after = expr_array_strings(v)?,
            "after_if" => {
                let pairs = expr_array_strings(v)?;
                for p in pairs {
                    let (lhs, rhs) = p.split_once('=').ok_or_else(|| {
                        syn::Error::new(v.span(), "after_if entry must be 'cond=dep'")
                    })?;
                    after_if.push((lhs.trim().to_string(), rhs.trim().to_string()));
                }
            }
            "default_label" => default_label = Some(lit_str(v)?),
            "core" => core = lit_bool(v)?,
            other => {
                return Err(syn::Error::new(
                    nv.path.span(),
                    format!("unknown Task attribute key '{other}'"),
                ));
            }
        }
    }

    let id = id.ok_or_else(|| syn::Error::new(struct_ident.span(), "Task: missing id"))?;
    let module =
        module.ok_or_else(|| syn::Error::new(struct_ident.span(), "Task: missing module"))?;
    let phase = phase.ok_or_else(|| syn::Error::new(struct_ident.span(), "Task: missing phase"))?;

    let config_ty = config_ty.unwrap_or_else(|| parse_quote!(#struct_ident));

    let config_path = match config_path {
        Some(p) => p,
        None => {
            // Default: <module>.steps.<step>, where step is the last segment of the task id.
            let prefix = format!("{module}.");
            if !id.starts_with(&prefix) {
                return Err(syn::Error::new(
                    struct_ident.span(),
                    "Task: config_path omitted but id does not start with '<module>.'",
                ));
            }
            let step = id
                .rsplit('.')
                .next()
                .ok_or_else(|| syn::Error::new(struct_ident.span(), "Task: invalid id"))?;
            format!("{module}.steps.{step}")
        }
    };

    let meta = TaskMeta {
        id,
        module,
        phase,
        config_path,
        provides,
        after,
        after_if,
        default_label: default_label
            .ok_or_else(|| syn::Error::new(struct_ident.span(), "Task: missing default_label"))?,
        core,
    };

    let id_lit = meta.id;
    let module_lit = meta.module;
    let phase_lit = meta.phase;
    let config_path_lit = meta.config_path;
    let provides_lits: Vec<_> = meta.provides.into_iter().collect();
    let after_lits: Vec<_> = meta.after.into_iter().collect();
    let default_label_lit = meta.default_label;
    let core_bool = meta.core;
    let after_if_pairs = meta.after_if;

    let after_if_stmts = after_if_pairs.iter().map(|(cond, dep)| {
        // Supported conditions:
        // - "<path>" (table exists)
        // - "enabled:<path>" (table exists AND enabled=true (default true))
        // - "any_enabled_under:<path>" (root exists AND at least one child table has enabled=true)
        if let Some(path) = cond.strip_prefix("enabled:") {
            quote! {
                if doc.has_table_path(#path) {
                    let enabled_path = format!("{}.enabled", #path);
                    let enabled = doc.value_path(&enabled_path).and_then(|v| v.as_bool()).unwrap_or(true);
                    if enabled {
                        after.push(#dep.to_string());
                    }
                }
            }
        } else if let Some(root) = cond.strip_prefix("any_enabled_under:") {
            quote! {
                if let Some(root_tbl) = doc.table_path(#root) {
                    let enabled = root_tbl.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                    if enabled {
                        let mut any = false;
                        for (_, v) in root_tbl {
                            let Some(child) = v.as_table() else { continue; };
                            let child_enabled = child.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                            if child_enabled {
                                any = true;
                                break;
                            }
                        }
                        if any {
                            after.push(#dep.to_string());
                        }
                    }
                }
            }
        } else {
            quote! {
                if doc.has_table_path(#cond) {
                    after.push(#dep.to_string());
                }
            }
        }
    });

    let core_check = if core_bool {
        quote! {
            if !cfg.enabled {
                return Err(crate::Error::msg(format!(
                    "[{}].enabled=false is not allowed (core step)",
                    #config_path_lit
                )));
            }
        }
    } else {
        quote! {
            if !cfg.enabled {
                return Ok(());
            }
        }
    };

    let expanded = quote! {
        #st

        impl #struct_ident {
            pub const ID: &'static str = #id_lit;
            pub const MODULE: &'static str = #module_lit;
            pub const PHASE: &'static str = #phase_lit;
            pub const CONFIG_PATH: &'static str = #config_path_lit;
            pub const CORE: bool = #core_bool;

            pub fn plan(doc: &crate::config::ConfigDoc, plan: &mut crate::planner::Plan) -> crate::Result<()> {
                let cfg: #config_ty = doc
                    .deserialize_path::<#config_ty>(#config_path_lit)?
                    .unwrap_or_default();

                #core_check

                let label = cfg
                    .label
                    .clone()
                    .unwrap_or_else(|| #default_label_lit.to_string());

                let mut after: Vec<String> = vec![#(#after_lits.to_string()),*];
                #(#after_if_stmts)*

                plan.add(crate::planner::Task{
                    id: #id_lit.to_string(),
                    label,
                    module: #module_lit.to_string(),
                    phase: #phase_lit.to_string(),
                    after,
                    provides: vec![#(#provides_lits.to_string()),*],
                })?;
                Ok(())
            }

            pub fn exec(doc: &crate::config::ConfigDoc, ctx: &mut crate::executor::ExecCtx) -> crate::Result<()> {
                let cfg: #config_ty = doc
                    .deserialize_path::<#config_ty>(#config_path_lit)?
                    .unwrap_or_default();

                #core_check

                if ctx.dry_run {
                    ctx.log(&format!("DRY-RUN: exec {}", #id_lit));
                    return Ok(());
                }

                // Tasks define their runtime behavior by implementing:
                // `fn run(cfg: &Self, doc: &crate::config::ConfigDoc, ctx: &mut crate::executor::ExecCtx) -> crate::Result<()>`
                Self::run(&cfg, doc, ctx)
            }
        }
    };

    Ok(expanded.into())
}

struct ModuleMeta {
    id: String,
    config_ty: syn::Type,
    config_path: String,
    tasks: Vec<syn::Path>,
}

fn module_impl(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut st: ItemStruct = syn::parse(item)?;
    st.attrs = drop_our_attrs(&st.attrs);
    let struct_ident = st.ident.clone();

    let parser = syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse(attr)?;

    let mut id: Option<String> = None;
    let mut config_ty: Option<syn::Type> = None;
    let mut config_path: Option<String> = None;
    let mut tasks: Option<Vec<syn::Path>> = None;

    for m in metas {
        let Meta::NameValue(nv) = m else {
            return Err(syn::Error::new(m.span(), "expected key = value"));
        };
        let Some(key) = nv.path.get_ident().map(|i| i.to_string()) else {
            return Err(syn::Error::new(nv.path.span(), "expected ident key"));
        };
        let v = &nv.value;
        match key.as_str() {
            "id" => id = Some(lit_str(v)?),
            "config" => config_ty = Some(expr_type(v)?),
            "config_path" => config_path = Some(lit_str(v)?),
            "tasks" => tasks = Some(expr_array_paths(v)?),
            other => {
                return Err(syn::Error::new(
                    nv.path.span(),
                    format!("unknown Module attribute key '{other}'"),
                ));
            }
        }
    }

    let meta = ModuleMeta {
        id: id.ok_or_else(|| syn::Error::new(struct_ident.span(), "Module: missing id"))?,
        config_ty: config_ty
            .ok_or_else(|| syn::Error::new(struct_ident.span(), "Module: missing config"))?,
        config_path: config_path
            .ok_or_else(|| syn::Error::new(struct_ident.span(), "Module: missing config_path"))?,
        tasks: tasks
            .ok_or_else(|| syn::Error::new(struct_ident.span(), "Module: missing tasks"))?,
    };

    let id_lit = meta.id;
    let config_ty = meta.config_ty;
    let config_path_lit = meta.config_path;
    let tasks = meta.tasks;

    let call_tasks = tasks.iter().map(|p| quote! { #p ::plan(doc, plan)?; });
    let reg_tasks = tasks.iter().map(|p| quote! { reg.add(#p::ID, #p::exec)?; });

    let expanded = quote! {
        #st

        impl crate::modules::Module for #struct_ident {
            fn id(&self) -> &'static str {
                #id_lit
            }

            fn detect(&self, doc: &crate::config::ConfigDoc) -> bool {
                doc.has_table_path(self.id())
            }

            fn plan(&self, doc: &crate::config::ConfigDoc, plan: &mut crate::planner::Plan) -> crate::Result<()> {
                let cfg: #config_ty = doc
                    .deserialize_path::<#config_ty>(#config_path_lit)?
                    .unwrap_or_default();
                if !cfg.enabled {
                    return Ok(());
                }

                #(#call_tasks)*
                Ok(())
            }
        }

        impl crate::executor::ModuleExec for #struct_ident {
            fn register_tasks(reg: &mut crate::executor::TaskRegistry) -> crate::Result<()> {
                #(#reg_tasks)*
                Ok(())
            }
        }
    };

    Ok(expanded.into())
}
