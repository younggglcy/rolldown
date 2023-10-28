pub mod commonjs_source_render;
pub mod esm_source_render;
pub mod esm_wrap_source_render;
pub mod scanner;
use oxc::span::{Atom, GetSpan, Span};
use rolldown_common::{ExportsKind, SymbolRef};
use rustc_hash::FxHashMap;
use string_wizard::{MagicString, UpdateOptions};

use super::{
  chunk_graph::ChunkGraph,
  graph::{graph::Graph, linker::LinkingInfo, symbols::get_symbol_final_name},
  module::{Module, NormalModule},
};

pub struct RendererContext<'ast> {
  graph: &'ast Graph,
  final_names: &'ast FxHashMap<SymbolRef, Atom>,
  source: &'ast mut MagicString<'static>,
  chunk_graph: &'ast ChunkGraph,
  module: &'ast NormalModule,
  linking_info: &'ast LinkingInfo,
  wrap_symbol_name: Option<&'ast Atom>,
  namespace_symbol_name: Option<&'ast Atom>,
  default_symbol_name: Option<&'ast Atom>,
  // Used to hoisted import declaration before the first statement
  first_stmt_start: Option<u32>,
}

impl<'ast> RendererContext<'ast> {
  #[allow(clippy::too_many_arguments)]
  pub fn new(
    graph: &'ast Graph,
    final_names: &'ast FxHashMap<SymbolRef, Atom>,
    source: &'ast mut MagicString<'static>,
    chunk_graph: &'ast ChunkGraph,
    module: &'ast NormalModule,
    linking_info: &'ast LinkingInfo,
  ) -> Self {
    let wrap_symbol_name =
      linking_info.wrap_symbol.and_then(|s| get_symbol_final_name(s, &graph.symbols, final_names));
    let namespace_symbol_name =
      get_symbol_final_name(module.namespace_symbol, &graph.symbols, final_names);
    let default_symbol_name = module
      .default_export_symbol
      .and_then(|s| get_symbol_final_name((module.id, s).into(), &graph.symbols, final_names));
    Self {
      graph,
      final_names,
      source,
      chunk_graph,
      module,
      linking_info,
      wrap_symbol_name,
      namespace_symbol_name,
      default_symbol_name,
      first_stmt_start: None,
    }
  }

  pub fn overwrite(&mut self, start: u32, end: u32, content: String) {
    self.source.update_with(
      start,
      end,
      content,
      UpdateOptions { overwrite: true, ..Default::default() },
    );
  }

  pub fn remove_node(&mut self, span: Span) {
    self.source.remove(span.start, span.end);
  }

  #[allow(clippy::needless_pass_by_value)]
  pub fn rename_symbol(&mut self, span: Span, name: Atom) {
    self.overwrite(span.start, span.end, name.to_string());
  }

  pub fn get_symbol_final_name(&self, symbol: SymbolRef) -> Option<&'ast Atom> {
    get_symbol_final_name(symbol, &self.graph.symbols, self.final_names)
  }

  pub fn get_runtime_symbol_final_name(&self, name: &Atom) -> &Atom {
    let symbol = self.graph.runtime.resolve_symbol(name);
    self.get_symbol_final_name(symbol).unwrap()
  }

  pub fn generate_namespace_variable_declaration(&mut self) -> Option<String> {
    if let Some(namespace_name) = self.namespace_symbol_name {
      let exports: String = self
        .linking_info
        .resolved_exports
        .iter()
        .map(|(exported_name, symbol_ref)| {
          let canonical_ref = self.graph.symbols.par_get_canonical_ref(*symbol_ref);
          let symbol = self.graph.symbols.get(canonical_ref);
          let return_expr = if let Some(ns_alias) = &symbol.namespace_alias {
            let canonical_ns_name = &self.final_names[&ns_alias.namespace_ref];
            format!("{canonical_ns_name}.{exported_name}",)
          } else {
            let canonical_name = self.final_names.get(&canonical_ref).unwrap();
            format!("{canonical_name}",)
          };
          format!("  get {exported_name}() {{ return {return_expr} }}",)
        })
        .collect::<Vec<_>>()
        .join(",\n");
      Some(format!("var {namespace_name} = {{\n{exports}\n}};\n",))
    } else {
      None
    }
  }

  pub fn generate_import_commonjs_module(
    &self,
    importee: &NormalModule,
    importee_linking_info: &LinkingInfo,
    with_namespace_init: bool,
  ) -> String {
    let wrap_symbol_name =
      self.get_symbol_final_name(importee_linking_info.wrap_symbol.unwrap()).unwrap();
    let to_esm_runtime_symbol_name = self.get_runtime_symbol_final_name(&"__toESM".into());
    let code = format!(
      "{to_esm_runtime_symbol_name}({wrap_symbol_name}(){})",
      if self.module.module_type.is_esm() { ", 1" } else { "" }
    );
    if with_namespace_init {
      let namespace_name = self.get_symbol_final_name(importee.namespace_symbol).unwrap();
      format!("var {namespace_name} = {code};\n")
    } else {
      code
    }
  }

  pub fn get_importee_by_span(&self, span: Span) -> &Module {
    &self.graph.modules[self.module.get_import_module_by_span(span)]
  }

  pub fn visit_binding_identifier(&mut self, ident: &'ast oxc::ast::ast::BindingIdentifier) {
    if let Some(name) =
      self.get_symbol_final_name((self.module.id, ident.symbol_id.get().unwrap()).into())
    {
      if ident.name != name {
        self.rename_symbol(ident.span, name.clone());
      }
    }
  }

  pub fn visit_identifier_reference(
    &mut self,
    ident: &'ast oxc::ast::ast::IdentifierReference,
    is_call: bool,
  ) {
    let Some(symbol_id) =
      self.graph.symbols.references_table[self.module.id][ident.reference_id.get().unwrap()]
    else {
      // This is global identifier references, eg `console.log`. We don't need to rewrite it.
      return;
    };
    let symbol_ref = (self.module.id, symbol_id).into();
    let symbol = self.graph.symbols.get(symbol_ref);
    if let Some(ns_alias) = &symbol.namespace_alias {
      // If import symbol from commonjs, the reference of the symbol is not resolved,
      // Here need write it to property access. eg `import { a } from 'cjs'; console.log(a)` => `console.log(cjs_ns.a)`
      // Note: we should rewrite call expression to indirect call, eg `import { a } from 'cjs'; console.log(a())` => `console.log((0, cjs_ns.a)())`
      let canonical_ns_name =
        get_symbol_final_name(ns_alias.namespace_ref, &self.graph.symbols, self.final_names)
          .unwrap();
      let property_name = &ns_alias.property_name;
      self.source.update(
        ident.span.start,
        ident.span.end,
        if is_call {
          format!("(0, {canonical_ns_name}.{property_name})",)
        } else {
          format!("{canonical_ns_name}.{property_name}",)
        },
      );
    } else if let Some(name) = self.get_symbol_final_name(symbol_ref) {
      if ident.name != name {
        self.rename_symbol(ident.span, name.clone());
      }
    }
  }

  pub fn visit_export_all_declaration(
    &mut self,
    decl: &'ast oxc::ast::ast::ExportAllDeclaration<'ast>,
  ) {
    if let Module::Normal(importee) = self.get_importee_by_span(decl.span) {
      if importee.exports_kind == ExportsKind::CommonJs {
        // __reExport(a_exports, __toESM(require_c()));
        let namespace_name = self.namespace_symbol_name.unwrap();
        let re_export_runtime_symbol_name =
          self.get_runtime_symbol_final_name(&"__reExport".into());
        self.source.update(
          decl.span.start,
          decl.span.end,
          format!(
            "{re_export_runtime_symbol_name}({namespace_name}, {});",
            self.generate_import_commonjs_module(
              importee,
              &self.graph.linking_infos[importee.id],
              false
            )
          ),
        );
        return;
      }
    }
    self.remove_node(decl.span);
  }

  pub fn visit_import_expression(&mut self, expr: &oxc::ast::ast::ImportExpression<'ast>) {
    if let oxc::ast::ast::Expression::StringLiteral(str) = &expr.source {
      if let Some(chunk_id) =
        self.chunk_graph.module_to_chunk[self.module.get_import_module_by_span(expr.span)]
      {
        let chunk = &self.chunk_graph.chunks[chunk_id];
        self.overwrite(
          str.span.start,
          str.span.end,
          // TODO: the path should be relative to the current importer chunk
          format!("'./{}'", chunk.file_name.as_ref().unwrap()),
        );
      } else {
        // external module doesn't belong to any chunk, just keep this as it is
      }
    }
  }

  pub fn visit_import_declaration(&mut self, decl: &'ast oxc::ast::ast::ImportDeclaration<'ast>) {
    self.remove_node(decl.span);
    let module_id = self.module.get_import_module_by_span(decl.span);
    let importee = &self.graph.modules[module_id];
    let importee_linking_info = &self.graph.linking_infos[module_id];
    let start = self.first_stmt_start.unwrap_or(decl.span.start);
    if let Module::Normal(importee) = importee {
      if importee.exports_kind == ExportsKind::CommonJs {
        self.source.append_right(
          start,
          self.generate_import_commonjs_module(
            importee,
            &self.graph.linking_infos[importee.id],
            true,
          ),
        );
      } else if let Some(wrap_symbol) = importee_linking_info.wrap_symbol {
        let wrap_symbol_name = self.get_symbol_final_name(wrap_symbol).unwrap();
        // init wrapped esm module
        self.source.append_right(start, format!("{wrap_symbol_name}();\n"));
      }
    }
  }

  pub fn visit_call_expression(&mut self, expr: &'ast oxc::ast::ast::CallExpression<'ast>) {
    if let oxc::ast::ast::Expression::Identifier(ident) = &expr.callee {
      if ident.name == "require" {
        if let Module::Normal(importee) = self.get_importee_by_span(expr.span) {
          let importee_linking_info = &self.graph.linking_infos[importee.id];
          let wrap_symbol_name =
            self.get_symbol_final_name(importee_linking_info.wrap_symbol.unwrap()).unwrap();
          if importee.exports_kind == ExportsKind::CommonJs {
            self.source.update(expr.span.start, expr.span.end, format!("{wrap_symbol_name}()"));
          } else {
            let namespace_name = self.get_symbol_final_name(importee.namespace_symbol).unwrap();
            let to_commonjs_runtime_symbol_name =
              self.get_runtime_symbol_final_name(&"__toCommonJS".into());
            self.source.update(
              expr.span.start,
              expr.span.end,
              format!(
                "({wrap_symbol_name}(), {to_commonjs_runtime_symbol_name}({namespace_name}))"
              ),
            );
          }
        }
      }
    }
  }

  fn visit_statement(&mut self, stmt: &'ast oxc::ast::ast::Statement<'ast>) {
    if !matches!(stmt, oxc::ast::ast::Statement::Declaration(_)) && self.first_stmt_start.is_none()
    {
      self.first_stmt_start = Some(stmt.span().start);
    }
  }
}
