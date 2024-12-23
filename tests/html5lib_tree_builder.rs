// A lot of this test harness has been copied from html5ever.
//
// Copyright 2014-2017 The html5ever Project Developers. See the
// COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use glob::glob;
use libtest_mimic::{self, Arguments, Trial};

use html5ever::interface::create_element;
use html5ever::tokenizer::states::{RawKind, State};
use html5ever::tree_builder::{TreeBuilder, TreeBuilderOpts};
use html5ever::{namespace_url, ns};
use html5ever::{LocalName, QualName};
use html5gum::emitters::html5ever::Html5everEmitter;
use html5gum::{testutils::trace_log, Tokenizer};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use pretty_assertions::assert_eq;

mod testutils;

#[derive(Default, Debug, Clone)]
struct Testcase {
    data: String,
    errors: Option<String>,
    new_errors: Option<String>,
    document_fragment: Option<String>,
    script_off: Option<String>,
    script_on: Option<String>,
    document: Option<String>,
}

impl Testcase {
    fn parse(path: &Path, iter: impl Iterator<Item = std::io::Result<Vec<u8>>>) -> Option<Self> {
        let mut rv = Testcase::default();
        let mut current_field: Option<&mut String> = Some(&mut rv.data);
        let mut has_errors = false;

        for line in iter {
            let line = String::from_utf8(line.unwrap()).unwrap();

            match line.as_str() {
                "#data" => {
                    if let Some(ref mut field) = current_field {
                        if field.ends_with("\n\n") {
                            field.pop();
                        }

                        if has_errors {
                            return Some(rv);
                        }
                    }
                }
                "#errors" => {
                    current_field = Some(rv.errors.get_or_insert_with(Default::default));
                    has_errors = true;
                }
                "#new-errors" => {
                    current_field = Some(rv.new_errors.get_or_insert_with(Default::default))
                }
                "#document-fragment" => {
                    current_field = Some(rv.document_fragment.get_or_insert_with(Default::default))
                }
                "#script-on" => {
                    current_field = Some(rv.script_on.get_or_insert_with(Default::default))
                }
                "#script-off" => {
                    current_field = Some(rv.script_off.get_or_insert_with(Default::default))
                }
                "#document" => {
                    current_field = Some(rv.document.get_or_insert_with(Default::default))
                }
                x => match current_field {
                    Some(ref mut current_field) => {
                        current_field.push_str(x);
                        current_field.push('\n');
                    }
                    None => {
                        panic!("{:?}: Unexpected character: {:?}", path, x);
                    }
                },
            }
        }

        if has_errors {
            Some(rv)
        } else {
            None
        }
    }
}

fn produce_testcases_from_file(tests: &mut Vec<Trial>, path: &Path) {
    let fname = path.file_name().unwrap().to_str().unwrap();

    let mut lines_iter = BufReader::new(File::open(path).unwrap())
        .split(b'\n')
        .peekable();

    let mut i = 0;

    while let Some(testcase) = Testcase::parse(path, &mut lines_iter) {
        i += 1;

        // if script_on is not explicitly provided, it's ok to run this test with scripting
        // disabled
        if testcase.script_on.is_none() {
            tests.push(build_test(testcase.clone(), fname, i, false));
        }

        // if script_off is not explicitly provided, it's ok to run this test with scripting
        // enabled
        if testcase.script_off.is_none() {
            tests.push(build_test(testcase, fname, i, true));
        }
    }
}

fn context_name(context: &str) -> QualName {
    if let Some(cx) = context.strip_prefix("svg ") {
        QualName::new(None, ns!(svg), LocalName::from(cx))
    } else if let Some(cx) = context.strip_prefix("math ") {
        QualName::new(None, ns!(mathml), LocalName::from(cx))
    } else {
        QualName::new(None, ns!(html), LocalName::from(context))
    }
}

fn map_tokenizer_state(input: State) -> html5gum::State {
    match input {
        State::Data => html5gum::State::Data,
        State::Plaintext => html5gum::State::PlainText,
        State::RawData(RawKind::Rcdata) => html5gum::State::RcData,
        State::RawData(RawKind::Rawtext) => html5gum::State::RawText,
        State::RawData(RawKind::ScriptData) => html5gum::State::ScriptData,
        x => todo!("{:?}", x),
    }
}

fn build_test(testcase: Testcase, fname: &str, i: usize, scripting: bool) -> Trial {
    let scripting_text = if scripting { "yesscript" } else { "noscript" };
    Trial::test(format!("{}:{}:{scripting_text}", fname, i), move || {
        testutils::catch_unwind_and_report(move || {
            trace_log(&format!("{:#?}", testcase));
            let mut rcdom = RcDom::default();
            let opts = TreeBuilderOpts {
                scripting_enabled: scripting,
                ..Default::default()
            };
            let initial_state;
            let mut tree_builder;

            if let Some(ref frag) = testcase.document_fragment {
                let frag = frag.trim_end_matches('\n');
                let context_name = context_name(frag);
                let context_element = create_element(&rcdom, context_name, Vec::new());
                tree_builder = TreeBuilder::new_for_fragment(rcdom, context_element, None, opts);
                initial_state = Some(map_tokenizer_state(
                    tree_builder.tokenizer_state_for_context_elem(),
                ));
            } else {
                tree_builder = TreeBuilder::new(rcdom, opts);
                initial_state = None;
            }

            let token_emitter = Html5everEmitter::new(&mut tree_builder);

            let input = &testcase.data[..testcase.data.len().saturating_sub(1)];
            let mut tokenizer = Tokenizer::new_with_emitter(input, token_emitter);
            if let Some(state) = initial_state {
                tokenizer.set_state(state);
            }

            tokenizer.finish().unwrap();

            let rcdom = tree_builder.sink;
            let mut actual = String::new();
            let root = rcdom.document.children.borrow();
            let root2 = if testcase.document_fragment.is_some() {
                // fragment case: serialize children of the html element
                // rather than children of the document
                root[0].children.borrow()
            } else {
                root
            };

            for child in root2.iter() {
                serialize(&mut actual, 1, child.clone());
            }

            let expected = testcase.document.unwrap();
            assert_eq!(actual, expected);
        })
    })
}

fn serialize(buf: &mut String, indent: usize, handle: Handle) {
    buf.push('|');
    buf.push_str(" ".repeat(indent).as_str());

    let node = handle;
    match node.data {
        NodeData::Document => panic!("should not reach Document"),

        NodeData::Doctype {
            ref name,
            ref public_id,
            ref system_id,
        } => {
            buf.push_str("<!DOCTYPE ");
            buf.push_str(name);
            if !public_id.is_empty() || !system_id.is_empty() {
                buf.push_str(&format!(" \"{}\" \"{}\"", public_id, system_id));
            }
            buf.push_str(">\n");
        }

        NodeData::Text { ref contents } => {
            buf.push('"');
            buf.push_str(&contents.borrow());
            buf.push_str("\"\n");
        }

        NodeData::Comment { ref contents } => {
            buf.push_str("<!-- ");
            buf.push_str(contents);
            buf.push_str(" -->\n");
        }

        NodeData::Element {
            ref name,
            ref attrs,
            ..
        } => {
            buf.push('<');
            match name.ns {
                ns!(svg) => buf.push_str("svg "),
                ns!(mathml) => buf.push_str("math "),
                _ => (),
            }
            buf.push_str(&name.local);
            buf.push_str(">\n");

            let mut attrs = attrs.borrow().clone();
            attrs.sort_by(|x, y| x.name.local.cmp(&y.name.local));
            // FIXME: sort by UTF-16 code unit

            for attr in attrs.into_iter() {
                buf.push('|');
                buf.push_str(" ".repeat(indent + 2).as_str());
                match attr.name.ns {
                    ns!(xlink) => buf.push_str("xlink "),
                    ns!(xml) => buf.push_str("xml "),
                    ns!(xmlns) => buf.push_str("xmlns "),
                    _ => (),
                }
                buf.push_str(&format!("{}=\"{}\"\n", attr.name.local, attr.value));
            }
        }

        NodeData::ProcessingInstruction { .. } => unreachable!(),
    }

    for child in node.children.borrow().iter() {
        serialize(buf, indent + 2, child.clone());
    }

    if let NodeData::Element {
        ref template_contents,
        ..
    } = node.data
    {
        if let Some(ref content) = &*template_contents.borrow() {
            buf.push('|');
            buf.push_str(" ".repeat(indent + 2).as_str());
            buf.push_str("content\n");
            for child in content.children.borrow().iter() {
                serialize(buf, indent + 4, child.clone());
            }
        }
    }
}

fn main() {
    let args = Arguments::from_args();
    let mut tests = Vec::new();

    for entry in glob("tests/custom-html5lib-tests/tree-construction/*.dat").unwrap() {
        produce_testcases_from_file(&mut tests, &entry.unwrap());
    }

    for entry in glob("tests/html5lib-tests/tree-construction/*.dat").unwrap() {
        produce_testcases_from_file(&mut tests, &entry.unwrap());
    }

    libtest_mimic::run(&args, tests).exit();
}
