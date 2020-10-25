// Copyright 2018-2020 the Deno authors. All rights reserved. MIT license.

use crate::media_type::MediaType;
use crate::tsc_config;

use deno_core::error::AnyError;
use deno_core::serde_json;
use deno_core::ModuleSpecifier;
use std::error::Error;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::RwLock;
use swc_common::chain;
use swc_common::comments::Comment;
use swc_common::comments::SingleThreadedComments;
use swc_common::errors::Diagnostic;
use swc_common::errors::DiagnosticBuilder;
use swc_common::errors::Emitter;
use swc_common::errors::Handler;
use swc_common::errors::HandlerFlags;
use swc_common::FileName;
use swc_common::Globals;
use swc_common::Loc;
use swc_common::SourceFile;
use swc_common::SourceMap;
use swc_common::Span;
use swc_ecmascript::ast::Module;
use swc_ecmascript::ast::Program;
use swc_ecmascript::codegen::text_writer::JsWriter;
use swc_ecmascript::codegen::Node;
use swc_ecmascript::dep_graph::analyze_dependencies;
use swc_ecmascript::dep_graph::DependencyDescriptor;
use swc_ecmascript::parser::lexer::Lexer;
use swc_ecmascript::parser::EsConfig;
use swc_ecmascript::parser::JscTarget;
use swc_ecmascript::parser::StringInput;
use swc_ecmascript::parser::Syntax;
use swc_ecmascript::parser::TsConfig;
use swc_ecmascript::transforms::fixer;
use swc_ecmascript::transforms::helpers;
use swc_ecmascript::transforms::pass::Optional;
use swc_ecmascript::transforms::proposals;
use swc_ecmascript::transforms::react;
use swc_ecmascript::transforms::typescript;
use swc_ecmascript::visit::FoldWith;

static TARGET: JscTarget = JscTarget::Es2020;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Location {
  pub filename: String,
  pub line: usize,
  pub col: usize,
}

impl Into<Location> for swc_common::Loc {
  fn into(self) -> Location {
    use swc_common::FileName::*;

    let filename = match &self.file.name {
      Real(path_buf) => path_buf.to_string_lossy().to_string(),
      Custom(str_) => str_.to_string(),
      _ => panic!("invalid filename"),
    };

    Location {
      filename,
      line: self.line,
      col: self.col_display,
    }
  }
}

impl Into<ModuleSpecifier> for Location {
  fn into(self) -> ModuleSpecifier {
    ModuleSpecifier::resolve_url_or_path(&self.filename).unwrap()
  }
}

impl std::fmt::Display for Location {
  fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    write!(f, "{}:{}:{}", self.filename, self.line, self.col)
  }
}

/// A buffer for collecting diagnostic messages from the AST parser.
#[derive(Debug)]
pub struct DiagnosticBuffer(Vec<String>);

impl Error for DiagnosticBuffer {}

impl fmt::Display for DiagnosticBuffer {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let s = self.0.join(",");
    f.pad(&s)
  }
}

impl DiagnosticBuffer {
  pub fn from_error_buffer<F>(error_buffer: ErrorBuffer, get_loc: F) -> Self
  where
    F: Fn(Span) -> Loc,
  {
    let s = error_buffer.0.read().unwrap().clone();
    let diagnostics = s
      .iter()
      .map(|d| {
        let mut msg = d.message();

        if let Some(span) = d.span.primary_span() {
          let loc = get_loc(span);
          let file_name = match &loc.file.name {
            FileName::Custom(n) => n,
            _ => unreachable!(),
          };
          msg = format!(
            "{} at {}:{}:{}",
            msg, file_name, loc.line, loc.col_display
          );
        }

        msg
      })
      .collect::<Vec<String>>();

    Self(diagnostics)
  }
}

/// A buffer for collecting errors from the AST parser.
#[derive(Debug, Clone)]
pub struct ErrorBuffer(Arc<RwLock<Vec<Diagnostic>>>);

impl ErrorBuffer {
  pub fn new() -> Self {
    Self(Arc::new(RwLock::new(Vec::new())))
  }
}

impl Emitter for ErrorBuffer {
  fn emit(&mut self, db: &DiagnosticBuilder) {
    self.0.write().unwrap().push((**db).clone());
  }
}

fn get_es_config(jsx: bool) -> EsConfig {
  EsConfig {
    class_private_methods: true,
    class_private_props: true,
    class_props: true,
    dynamic_import: true,
    export_default_from: true,
    export_namespace_from: true,
    import_meta: true,
    jsx,
    nullish_coalescing: true,
    num_sep: true,
    optional_chaining: true,
    top_level_await: true,
    ..EsConfig::default()
  }
}

fn get_ts_config(tsx: bool, dts: bool) -> TsConfig {
  TsConfig {
    decorators: true,
    dts,
    dynamic_import: true,
    tsx,
    ..TsConfig::default()
  }
}

pub fn get_syntax(media_type: &MediaType) -> Syntax {
  match media_type {
    MediaType::JavaScript => Syntax::Es(get_es_config(false)),
    MediaType::JSX => Syntax::Es(get_es_config(true)),
    MediaType::TypeScript => Syntax::Typescript(get_ts_config(false, false)),
    MediaType::Dts => Syntax::Typescript(get_ts_config(false, true)),
    MediaType::TSX => Syntax::Typescript(get_ts_config(true, false)),
    _ => Syntax::Es(get_es_config(false)),
  }
}

/// Options which can be adjusted when transpiling a module.
#[derive(Debug, Clone)]
pub struct EmitOptions {
  /// Indicate if JavaScript is being checked/transformed as well, or if it is
  /// only TypeScript.
  pub check_js: bool,
  /// When emitting a legacy decorator, also emit experimental decorator meta
  /// data.  Defaults to `false`.
  pub emit_metadata: bool,
  /// Should the source map be inlined in the emitted code file, or provided
  /// as a separate file.  Defaults to `true`.
  pub inline_source_map: bool,
  /// When transforming JSX, what value should be used for the JSX factory.
  /// Defaults to `React.createElement`.
  pub jsx_factory: String,
  /// When transforming JSX, what value should be used for the JSX fragment
  /// factory.  Defaults to `React.Fragment`.
  pub jsx_fragment_factory: String,
  /// Should JSX be transformed or preserved.  Defaults to `true`.
  pub transform_jsx: bool,
}

impl Default for EmitOptions {
  fn default() -> Self {
    EmitOptions {
      check_js: false,
      emit_metadata: false,
      inline_source_map: true,
      jsx_factory: "React.createElement".into(),
      jsx_fragment_factory: "React.Fragment".into(),
      transform_jsx: true,
    }
  }
}

impl From<tsc_config::TsConfig> for EmitOptions {
  fn from(config: tsc_config::TsConfig) -> Self {
    let options: tsc_config::EmitConfigOptions =
      serde_json::from_value(config.0).unwrap();
    EmitOptions {
      check_js: options.check_js,
      emit_metadata: options.emit_decorator_metadata,
      inline_source_map: true,
      jsx_factory: options.jsx_factory,
      jsx_fragment_factory: options.jsx_fragment_factory,
      transform_jsx: options.jsx == "react",
    }
  }
}

/// A logical structure to hold the value of a parsed module for further
/// processing.
#[derive(Clone)]
pub struct ParsedModule {
  pub(crate) comments: SingleThreadedComments,
  leading_comments: Vec<Comment>,
  pub(crate) module: Module,
  pub(crate) source_map: Rc<SourceMap>,
}

impl fmt::Debug for ParsedModule {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    f.debug_struct("ParsedModule")
      .field("comments", &self.comments)
      .field("leading_comments", &self.leading_comments)
      .field("module", &self.module)
      .finish()
  }
}

impl ParsedModule {
  /// Return a vector of dependencies for the module.
  pub fn analyze_dependencies(&self) -> Vec<DependencyDescriptor> {
    analyze_dependencies(&self.module, &self.source_map, &self.comments)
  }

  /// Get the module's leading comments, where triple slash directives might
  /// be located.
  pub fn get_leading_comments(&self) -> Vec<Comment> {
    self.leading_comments.clone()
  }

  /// Get a location for a given span within the module.
  pub fn get_location(&self, span: &Span) -> Location {
    self.source_map.lookup_char_pos(span.lo).into()
  }

  /// Transform a TypeScript file into a JavaScript file, based on the supplied
  /// options.
  ///
  /// The result is a tuple of the code and optional source map as strings.
  pub fn transpile(
    self,
    options: &EmitOptions,
  ) -> Result<(String, Option<String>), AnyError> {
    let program = Program::Module(self.module);

    let jsx_pass = react::react(
      self.source_map.clone(),
      Some(&self.comments),
      react::Options {
        pragma: options.jsx_factory.clone(),
        pragma_frag: options.jsx_fragment_factory.clone(),
        // this will use `Object.assign()` instead of the `_extends` helper
        // when spreading props.
        use_builtins: true,
        ..Default::default()
      },
    );
    let mut passes = chain!(
      Optional::new(jsx_pass, options.transform_jsx),
      proposals::decorators::decorators(proposals::decorators::Config {
        legacy: true,
        emit_metadata: options.emit_metadata
      }),
      typescript::strip(),
      fixer(Some(&self.comments)),
    );

    let program = swc_common::GLOBALS.set(&Globals::new(), || {
      helpers::HELPERS.set(&helpers::Helpers::new(false), || {
        program.fold_with(&mut passes)
      })
    });

    let mut src_map_buf = vec![];
    let mut buf = vec![];
    {
      let writer = Box::new(JsWriter::new(
        self.source_map.clone(),
        "\n",
        &mut buf,
        Some(&mut src_map_buf),
      ));
      let config = swc_ecmascript::codegen::Config { minify: false };
      let mut emitter = swc_ecmascript::codegen::Emitter {
        cfg: config,
        comments: Some(&self.comments),
        cm: self.source_map.clone(),
        wr: writer,
      };
      program.emit_with(&mut emitter)?;
    }
    let mut src = String::from_utf8(buf)?;
    let mut map: Option<String> = None;
    {
      let mut buf = Vec::new();
      self
        .source_map
        .build_source_map_from(&mut src_map_buf, None)
        .to_writer(&mut buf)?;

      if options.inline_source_map {
        src.push_str("//# sourceMappingURL=data:application/json;base64,");
        let encoded_map = base64::encode(buf);
        src.push_str(&encoded_map);
      } else {
        map = Some(String::from_utf8(buf)?);
      }
    }
    Ok((src, map))
  }
}

/// For a given specifier, source, and media type, parse the source of the
/// module and return a representation which can be further processed.
///
/// # Arguments
///
/// - `specifier` - The module specifier for the module.
/// - `source` - The source code for the module.
/// - `media_type` - The media type for the module.
///
pub fn parse(
  specifier: &ModuleSpecifier,
  source: &str,
  media_type: &MediaType,
) -> Result<ParsedModule, AnyError> {
  let source_map = SourceMap::default();
  let source_file = source_map.new_source_file(
    FileName::Custom(specifier.to_string()),
    source.to_string(),
  );
  let error_buffer = ErrorBuffer::new();
  let syntax = get_syntax(media_type);
  let input = StringInput::from(&*source_file);
  let comments = SingleThreadedComments::default();

  let handler = Handler::with_emitter_and_flags(
    Box::new(error_buffer.clone()),
    HandlerFlags {
      can_emit_warnings: true,
      dont_buffer_diagnostics: true,
      ..HandlerFlags::default()
    },
  );

  let lexer = Lexer::new(syntax, TARGET, input, Some(&comments));
  let mut parser = swc_ecmascript::parser::Parser::new_from(lexer);

  let sm = &source_map;
  let module = parser.parse_module().map_err(move |err| {
    let mut diagnostic = err.into_diagnostic(&handler);
    diagnostic.emit();

    DiagnosticBuffer::from_error_buffer(error_buffer, |span| {
      sm.lookup_char_pos(span.lo)
    })
  })?;
  let leading_comments =
    comments.with_leading(module.span.lo, |comments| comments.to_vec());

  Ok(ParsedModule {
    leading_comments,
    module,
    source_map: Rc::new(source_map),
    comments,
  })
}

/// A low level function which transpiles a source module into an swc
/// SourceFile.
pub fn transpile_module(
  filename: &str,
  src: &str,
  media_type: &MediaType,
  emit_options: &EmitOptions,
  cm: Rc<SourceMap>,
) -> Result<(Rc<SourceFile>, Module), AnyError> {
  // TODO(@kitsonk) DRY-up with ::parse()
  let error_buffer = ErrorBuffer::new();
  let handler = Handler::with_emitter_and_flags(
    Box::new(error_buffer.clone()),
    HandlerFlags {
      can_emit_warnings: true,
      dont_buffer_diagnostics: true,
      ..HandlerFlags::default()
    },
  );
  let comments = SingleThreadedComments::default();
  let syntax = get_syntax(media_type);
  let source_file =
    cm.new_source_file(FileName::Custom(filename.to_string()), src.to_string());
  let lexer = Lexer::new(
    syntax,
    TARGET,
    StringInput::from(&*source_file),
    Some(&comments),
  );
  let mut parser = swc_ecmascript::parser::Parser::new_from(lexer);
  let sm = cm.clone();
  let module = parser.parse_module().map_err(move |err| {
    let mut diagnostic = err.into_diagnostic(&handler);
    diagnostic.emit();

    DiagnosticBuffer::from_error_buffer(error_buffer, |span| {
      sm.lookup_char_pos(span.lo)
    })
  })?;
  // TODO(@kitsonk) DRY-up with ::transpile()
  let jsx_pass = react::react(
    cm,
    Some(&comments),
    react::Options {
      pragma: emit_options.jsx_factory.clone(),
      pragma_frag: emit_options.jsx_fragment_factory.clone(),
      // this will use `Object.assign()` instead of the `_extends` helper
      // when spreading props.
      use_builtins: true,
      ..Default::default()
    },
  );
  let mut passes = chain!(
    Optional::new(jsx_pass, emit_options.transform_jsx),
    proposals::decorators::decorators(proposals::decorators::Config {
      legacy: true,
      emit_metadata: emit_options.emit_metadata
    }),
    typescript::strip(),
    fixer(Some(&comments)),
  );
  let module = module.fold_with(&mut passes);

  Ok((source_file, module))
}

pub struct BundleHook;

impl swc_bundler::Hook for BundleHook {
  fn get_import_meta_url(
    &self,
    span: swc_common::Span,
    file: &swc_common::FileName,
  ) -> Result<Option<swc_ecmascript::ast::Expr>, AnyError> {
    // we use custom file names, and swc "wraps" these in `<` and `>` so, we
    // want to strip those back out.
    let mut value = file.to_string();
    value.pop();
    value.remove(0);
    Ok(Some(swc_ecmascript::ast::Expr::Lit(
      swc_ecmascript::ast::Lit::Str(swc_ecmascript::ast::Str {
        span,
        value: value.into(),
        has_escape: false,
      }),
    )))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use swc_ecmascript::dep_graph::DependencyKind;

  #[test]
  fn test_parsed_module_analyze_dependencies() {
    let specifier =
      ModuleSpecifier::resolve_url_or_path("https://deno.land/x/mod.js")
        .unwrap();
    let source = r#"import * as bar from "./test.ts";
    const foo = await import("./foo.ts");
    "#;
    let parsed_module = parse(&specifier, source, &MediaType::JavaScript)
      .expect("could not parse module");
    let actual = parsed_module.analyze_dependencies();
    assert_eq!(
      actual,
      vec![
        DependencyDescriptor {
          kind: DependencyKind::Import,
          is_dynamic: false,
          leading_comments: Vec::new(),
          col: 0,
          line: 1,
          specifier: "./test.ts".into()
        },
        DependencyDescriptor {
          kind: DependencyKind::Import,
          is_dynamic: true,
          leading_comments: Vec::new(),
          col: 22,
          line: 2,
          specifier: "./foo.ts".into()
        }
      ]
    );
  }

  #[test]
  fn test_transpile() {
    let specifier =
      ModuleSpecifier::resolve_url_or_path("https://deno.land/x/mod.ts")
        .expect("could not resolve specifier");
    let source = r#"
    enum D {
      A,
      B,
      C,
    }

    export class A {
      private b: string;
      protected c: number = 1;
      e: "foo";
      constructor (public d = D.A) {
        const e = "foo" as const;
        this.e = e;
      }
    }
    "#;
    let module = parse(&specifier, source, &MediaType::TypeScript)
      .expect("could not parse module");
    let (code, maybe_map) = module
      .transpile(&EmitOptions::default())
      .expect("could not strip types");
    assert!(code.starts_with("var D;\n(function(D) {\n"));
    assert!(
      code.contains("\n//# sourceMappingURL=data:application/json;base64,")
    );
    assert!(maybe_map.is_none());
  }

  #[test]
  fn test_transpile_tsx() {
    let specifier =
      ModuleSpecifier::resolve_url_or_path("https://deno.land/x/mod.ts")
        .expect("could not resolve specifier");
    let source = r#"
    export class A {
      render() {
        return <div><span></span></div>
      }
    }
    "#;
    let module = parse(&specifier, source, &MediaType::TSX)
      .expect("could not parse module");
    let (code, _) = module
      .transpile(&EmitOptions::default())
      .expect("could not strip types");
    assert!(code.contains("React.createElement(\"div\", null"));
  }

  #[test]
  fn test_transpile_decorators() {
    let specifier =
      ModuleSpecifier::resolve_url_or_path("https://deno.land/x/mod.ts")
        .expect("could not resolve specifier");
    let source = r#"
    function enumerable(value: boolean) {
      return function (
        _target: any,
        _propertyKey: string,
        descriptor: PropertyDescriptor,
      ) {
        descriptor.enumerable = value;
      };
    }
    
    export class A {
      @enumerable(false)
      a() {
        Test.value;
      }
    }
    "#;
    let module = parse(&specifier, source, &MediaType::TypeScript)
      .expect("could not parse module");
    let (code, _) = module
      .transpile(&EmitOptions::default())
      .expect("could not strip types");
    assert!(code.contains("_applyDecoratedDescriptor("));
  }
}
