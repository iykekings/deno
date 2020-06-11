use regex::Regex;
use std::ffi::OsStr;
use std::path::Path;
use url::Url;

use deno_core::ErrBox;
use deno_core::ModuleSpecifier;

use crate::doc::parser::DocParser;
use crate::flags::Flags;
use crate::global_state::GlobalState;
use crate::test_runner::is_supported as is_supported_test;

lazy_static! {
  static ref IMPORT_PATTERN: Regex = Regex::new(r"import[^(].*\n").unwrap();
  static ref EXAMPLE_PATTERN: Regex =
    Regex::new(r"@example\s*(?:<\w+>.*</\w+>)*\n(?:\s*\*\s*\n*)*```").unwrap();
  static ref TEST_TAG_PATTERN: Regex =
    Regex::new(r"@example\s*(?:<\w+>.*</\w+>)*\n(?:\s*\*\s*\n*)*```(\w+)")
      .unwrap();
  static ref AWAIT_PATTERN: Regex = Regex::new(r"\Wawait\s").unwrap();
  static ref TICKS_OR_IMPORT_PATTERN: Regex =
    Regex::new(r"(?:import[^(].*)|(?:```\w*)").unwrap();
  static ref CAPTION_PATTERN: Regex =
    Regex::new(r"<caption>([\s\w\W]+)</caption>").unwrap();
}

#[derive(Debug, Clone)]
struct DocTestNew {
  imports: Vec<String>,
  caption: Option<String>,
  line_number: usize,
  filename: String,
  source_code: String,
  ignore: bool,
  is_async: bool,
}

#[derive(Debug, Clone)]
struct JSDocExample {
  source_code: String,
  filename: String,
  line_number: usize,
}

impl JSDocExample {
  fn from_str(
    jsdoc_str: String,
    filename: String,
    line_number: usize,
  ) -> Vec<Self> {
    EXAMPLE_PATTERN
      .find_iter(&jsdoc_str)
      .filter_map(|cap| {
        jsdoc_str[cap.end()..].find("```").map(|i| JSDocExample {
          source_code: jsdoc_str[cap.start()..i + cap.end()].to_owned(),
          filename: filename.clone(),
          line_number,
        })
      })
      .collect()
  }

  fn parse(&self) -> Option<DocTestNew> {
    let test_tag = TEST_TAG_PATTERN
      .captures(&self.source_code)
      .and_then(|m| m.get(1).map(|c| c.as_str()));

    if test_tag == Some("text") {
      return None;
    }

    let mut imports: Vec<String> = IMPORT_PATTERN
      .captures_iter(&self.source_code)
      .filter_map(|caps| caps.get(0).and_then(|m| Some(m.as_str())))
      .map(String::from)
      .collect();
    imports.dedup();

    let caption = get_caption_from_example(&self.source_code);
    let source_code = get_code_from_example(&self.source_code);
    let is_async = AWAIT_PATTERN.find(&self.source_code).is_some();
    Some(DocTestNew {
      imports,
      caption,
      line_number: self.line_number,
      filename: self.filename.clone(),
      source_code,
      ignore: test_tag == Some("ignore"),
      is_async,
    })
  }
}

pub fn is_supported(path: &Path) -> bool {
  let valid_ext = ["ts", "tsx", "js", "jsx"];
  path
    .extension()
    .and_then(OsStr::to_str)
    .map(|ext| valid_ext.contains(&ext) && !is_supported_test(path))
    .unwrap_or(false)
}

fn render_doctestnew_file(
  doctests: Vec<DocTestNew>,
  fail_fast: bool,
  quiet: bool,
  filter: Option<String>,
) -> String {
  let mut test_file = "".to_string();

  let default_import = format!(
    "import {{ 
    assert,
    assertArrayContains,
    assertEquals,
    assertMatch,
    assertNotEquals,
    assertStrContains,
    assertStrictEq,
    assertThrows,
    assertThrowsAsync,
    equal,
    unimplemented,
    unreachable,
   }} from \"https://deno.land/std@{}/testing/asserts.ts\";\n",
    "0.50.0"
  );

  test_file.push_str(&default_import);
  let mut all_imports = doctests
    .iter()
    .map(|doctest| doctest.imports.clone())
    .flatten()
    .collect::<Vec<_>>();
  all_imports.dedup();

  test_file.push_str(&all_imports.join("\n"));
  // consider removing
  test_file.push_str("\n");

  let all_test_section = doctests
      .into_iter()
      .map(|doctest| {
          let async_str = if doctest.is_async {"async "} else {""};
          format!(
              "Deno.doctest({{\n\tname: \"{} - {} (line {})\",\n\tignore: {},\n\t{}fn() {{\n{}\n}}\n}});\n",
              // process this
              doctest.filename,
              doctest.caption.unwrap_or_else(|| "".to_string()),
              doctest.line_number,
              doctest.ignore,
              async_str,
              doctest.source_code
          )
      })
      .collect::<Vec<_>>()
      .join("\n");

  test_file.push_str(&all_test_section);

  let options = if let Some(filter) = filter {
    json!({ "failFast": fail_fast, "reportToConsole": !quiet, "disableLog": quiet, "isDoctest": true, "filter": filter })
  } else {
    json!({ "failFast": fail_fast, "reportToConsole": !quiet, "disableLog": quiet, "isDoctest": true })
  };

  let run_tests_cmd = format!(
    "\n// @ts-ignore\nDeno[Deno.internal].runTests({});\n",
    options
  );

  test_file.push_str(&run_tests_cmd);

  test_file
}

fn get_caption_from_example(ex: &str) -> Option<String> {
  CAPTION_PATTERN
    .captures(ex)
    .and_then(|cap| cap.get(1).map(|m| m.as_str()))
    .map(|caption| caption.to_string())
}

fn get_code_from_example(ex: &str) -> String {
  TICKS_OR_IMPORT_PATTERN
    .replace_all(ex, "\n")
    .lines()
    .skip(1)
    .filter_map(|line| {
      // consider removing this line
      let res = if line.trim_start().starts_with('*') {
        line.replacen("*", "", 1).trim_start().to_string()
      } else {
        line.trim_start().to_string()
      };
      match res.len() {
        0 => None,
        _ => Some(format!("  {}", res)),
      }
    })
    .collect::<Vec<_>>()
    .join("\n")
}

pub async fn prepare_doctests_new(
  files: Vec<Url>,
  flags: Flags,
  fail_fast: bool,
  quiet: bool,
  filter: Option<String>,
) -> Result<String, ErrBox> {
  let global_state = GlobalState::new(flags.clone())?;

  let loader = Box::new(global_state.file_fetcher.clone());
  let doc_parser = DocParser::new(loader);
  let mut tests: Vec<DocTestNew> = vec![];

  for file in files {
    let specifier = ModuleSpecifier::from(file);
    let parse_result = doc_parser.parse(&specifier.to_string()).await;
    let doc_nodes = match parse_result {
      Ok(nodes) => nodes,
      Err(e) => {
        eprintln!("{}", e);
        std::process::exit(1);
      }
    };
    let file_doctests = doc_nodes
      .iter()
      .filter_map(|node| {
        node.clone().js_doc.map(|c| (node.location.clone(), c))
      })
      .flat_map(|(loc, jsdoc_str)| {
        JSDocExample::from_str(jsdoc_str, loc.filename, loc.line)
      })
      .filter_map(|example| example.parse())
      .collect::<Vec<_>>();
    println!("doctest len: {}", file_doctests.len());
    tests.extend(file_doctests);
  }
  let test_str = render_doctestnew_file(tests, fail_fast, quiet, filter);
  Ok(test_str)
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn test_extract_jsdoc() {
    let jsdoc_str = r#"
@param list - LinkedList<T>
@example <caption>Linkedlists.compareWith</caption>
```ts
import { LinkedList } from './js_test/linkedlist.ts'
const testArr = [1, 2, 3, 4, 5, 6, 78, 9, 0, 65];
const firstList = new LinkedList<number>();
const secondList = new LinkedList<number>();
for (let data of testArr) {
  firstList.insertNode(data);
  secondList.insertNode(data);
}
const result = firstList.compareWith(secondList);
assert(result);
```"#;
    let res =
      JSDocExample::from_str(jsdoc_str.to_string(), "file".to_string(), 6);
    assert!(res.len() == 1);
    let doctest = res[0].parse();
    assert!(doctest.is_some());
    let doctest = doctest.unwrap();
    assert_eq!(1, doctest.imports.len());
    assert!(!doctest.is_async);
    assert!(!doctest.ignore);
    assert_eq!(doctest.caption, Some("Linkedlists.compareWith".to_string()));
    assert_eq!(
      doctest.source_code,
      vec![
        "  const testArr = [1, 2, 3, 4, 5, 6, 78, 9, 0, 65];",
        "  const firstList = new LinkedList<number>();",
        "  const secondList = new LinkedList<number>();",
        "  for (let data of testArr) {",
        "  firstList.insertNode(data);",
        "  secondList.insertNode(data);",
        "  }",
        "  const result = firstList.compareWith(secondList);",
        "  assert(result);"
      ]
      .join("\n")
    )
  }

  #[test]
  fn test_multiple_examples() {
    let jsdoc_str = r#"
@param fn - (data: T, index: number) => T
@example <caption>Linkedlist.map</caption>
```ts
import { LinkedList } from './js_test/linkedlist.ts';
import { LinkedList1 } from './js_test/linkedlist1.ts';
const testArr = [1, 2, 3, 4, 5, 6, 78, 9, 0, 65];
const testList = new LinkedList<number>();
for (let data of testArr) {
 testList.insertNode(data);
}
testList.map((c: number) => c ** 2);
testList.forEach((c: number, i: number) => assertEquals(c, testArr[i] ** 2));
```
@example <caption>Linkedlist.map 2</caption>
```ignore
import { LinkedList } from './js_test/linkedlist.ts'
const testArr = [1, 2, 3, 4, 5];
const testList = new LinkedList<number>();
for (let data of testArr) {
 testList.insertNode(data);
}
testList.map((c: number) => c ** 2);
testList.forEach((c: number, i: number) => assertEquals(c, testArr[i] ** 2));
```"#;
    let res =
      JSDocExample::from_str(jsdoc_str.to_string(), "file".to_string(), 6);
    assert!(res.len() == 2);
    let doctest = res[0].parse();
    assert!(doctest.is_some());
    let doctest = doctest.unwrap();
    assert_eq!(2, doctest.imports.len());
    assert!(!doctest.is_async);
    assert!(!doctest.ignore);
    assert_eq!(doctest.caption, Some("Linkedlist.map".to_string()));
    assert_eq!(
      doctest.source_code,
      vec![
        "  const testArr = [1, 2, 3, 4, 5, 6, 78, 9, 0, 65];",
        "  const testList = new LinkedList<number>();",
        "  for (let data of testArr) {",
        "  testList.insertNode(data);",
        "  }",
        "  testList.map((c: number) => c ** 2);",
        "  testList.forEach((c: number, i: number) => assertEquals(c, testArr[i] ** 2));",
  ].join("\n"));

    let doctest = res[1].parse();
    assert!(doctest.is_some());
    let doctest = doctest.unwrap();
    assert!(!doctest.is_async);
    assert!(doctest.ignore);
    assert_eq!(doctest.caption, Some("Linkedlist.map 2".to_string()));
    assert_eq!(
      doctest.source_code,
      vec![
        "  const testArr = [1, 2, 3, 4, 5];",
        "  const testList = new LinkedList<number>();",
        "  for (let data of testArr) {",
        "  testList.insertNode(data);",
        "  }",
        "  testList.map((c: number) => c ** 2);",
        "  testList.forEach((c: number, i: number) => assertEquals(c, testArr[i] ** 2));"
      ]
      .join("\n")
    );
  }

  #[test]
  fn test_code_without_jsdoc() {
    let jsdoc_str = r#"class Node<T> {
      constructor(public data: T, public next?: Node<T>) {}

      swap(other: Node<T>) {
        let temp = this.data;
        this.data = other.data;
        other.data = temp;
      }
    }"#;
    let res =
      JSDocExample::from_str(jsdoc_str.to_string(), "filename".to_string(), 5);
    assert!(res.is_empty());
  }

  #[test]
  fn test_async_detection() {
    let example = r#"@example
```ts
const response = await fetch("https://deno.land");
const body = await response.text();
assert(body.length > 0);
```"#;

    let res = JSDocExample {
      source_code: example.to_string(),
      filename: "filename".to_string(),
      line_number: 1,
    };
    let res = res.parse();
    assert!(res.is_some());
    let doctest = res.unwrap();
    assert!(doctest.is_async);
    assert!(!doctest.ignore);
    assert!(doctest.caption.is_none());
    assert_eq!(
      doctest.source_code,
      vec![
        "  const response = await fetch(\"https://deno.land\");",
        "  const body = await response.text();",
        "  assert(body.length > 0);",
      ]
      .join("\n")
    );
  }

  #[test]
  fn test_text_tag() {
    let example = r#"@example
```text
const response = await fetch("https://deno.land");
const body = await response.text();
assert(body.length > 0);
```"#;

    let res = JSDocExample {
      source_code: example.to_string(),
      filename: "file".to_string(),
      line_number: 0,
    };
    assert!(res.parse().is_none());
  }

  #[test]
  fn test_jump_example_without_backticks() {
    let jsdoc_str = r#"@example
const response = await fetch("https://deno.land");
const body = await response.text();
assert(body.length > 0);
"#;
    let doctest =
      JSDocExample::from_str(jsdoc_str.to_string(), "filename".to_string(), 0);
    assert!(doctest.is_empty());
  }
}
