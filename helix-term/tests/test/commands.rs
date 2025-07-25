use helix_term::application::Application;

use super::*;

mod insert;
mod movement;
mod write;

#[tokio::test(flavor = "multi_thread")]
async fn search_selection_detect_word_boundaries_at_eof() -> anyhow::Result<()> {
    // <https://github.com/helix-editor/helix/issues/12609>
    test((
        indoc! {"\
            #[o|]#ne
            two
            three"},
        "gej*h",
        indoc! {"\
            one
            two
            three#[
            |]#"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_selection_duplication() -> anyhow::Result<()> {
    // Forward
    test((
        indoc! {"\
            #[lo|]#rem
            ipsum
            dolor
            "},
        "CC",
        indoc! {"\
            #(lo|)#rem
            #(ip|)#sum
            #[do|]#lor
            "},
    ))
    .await?;

    // Backward
    test((
        indoc! {"\
            #[|lo]#rem
            ipsum
            dolor
            "},
        "CC",
        indoc! {"\
            #(|lo)#rem
            #(|ip)#sum
            #[|do]#lor
            "},
    ))
    .await?;

    // Copy the selection to previous line, skipping the first line in the file
    test((
        indoc! {"\
            test
            #[testitem|]#
            "},
        "<A-C>",
        indoc! {"\
            test
            #[testitem|]#
            "},
    ))
    .await?;

    // Copy the selection to previous line, including the first line in the file
    test((
        indoc! {"\
            test
            #[test|]#
            "},
        "<A-C>",
        indoc! {"\
            #[test|]#
            #(test|)#
            "},
    ))
    .await?;

    // Copy the selection to next line, skipping the last line in the file
    test((
        indoc! {"\
            #[testitem|]#
            test
            "},
        "C",
        indoc! {"\
            #[testitem|]#
            test
            "},
    ))
    .await?;

    // Copy the selection to next line, including the last line in the file
    test((
        indoc! {"\
            #[test|]#
            test
            "},
        "C",
        indoc! {"\
            #(test|)#
            #[test|]#
            "},
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_goto_file_impl() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;

    fn match_paths(app: &Application, matches: Vec<&str>) -> usize {
        app.editor
            .documents()
            .filter_map(|d| d.path()?.file_name())
            .filter(|n| matches.iter().any(|m| *m == n.to_string_lossy()))
            .count()
    }

    // Single selection
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("ione.js<esc>%gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // Multiple selection
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("ione.js<ret>two.js<esc>%<A-s>gf"),
        Some(&|app| {
            assert_eq!(2, match_paths(app, vec!["one.js", "two.js"]));
        }),
        false,
    )
    .await?;

    // Cursor on first quote
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js'<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // Cursor on last quote
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js'<esc>bgf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // ';' is behind the path
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js';<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // allow numeric values in path
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one123.js'<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one123.js"]));
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multi_selection_paste() -> anyhow::Result<()> {
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "yp",
        indoc! {"\
            lorem#[|lorem]#
            ipsum#(|ipsum)#
            dolor#(|dolor)#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multi_selection_shell_commands() -> anyhow::Result<()> {
    // pipe
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "|echo foo<ret>",
        indoc! {"\
            #[|foo]#
            #(|foo)#
            #(|foo)#"
        },
    ))
    .await?;

    // insert-output
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "!echo foo<ret>",
        indoc! {"\
            #[|foo]#lorem
            #(|foo)#ipsum
            #(|foo)#dolor
            "},
    ))
    .await?;

    // append-output
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "<A-!>echo foo<ret>",
        indoc! {"\
            lorem#[|foo]#
            ipsum#(|foo)#
            dolor#(|foo)#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_undo_redo() -> anyhow::Result<()> {
    // A jumplist selection is created at a point which is undone.
    //
    // * 2[<space>   Add two newlines at line start. We're now on line 3.
    // * <C-s>       Save the selection on line 3 in the jumplist.
    // * u           Undo the two newlines. We're now on line 1.
    // * <C-o><C-i>  Jump forward an back again in the jumplist. This would panic
    //               if the jumplist were not being updated correctly.
    test((
        "#[|]#",
        "2[<space><C-s>u<C-o><C-i>",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // A jumplist selection is passed through an edit and then an undo and then a redo.
    //
    // * [<space>    Add a newline at line start. We're now on line 2.
    // * <C-s>       Save the selection on line 2 in the jumplist.
    // * kd          Delete line 1. The jumplist selection should be adjusted to the new line 1.
    // * uU          Undo and redo the `kd` edit.
    // * <C-o>       Jump back in the jumplist. This would panic if the jumplist were not being
    //               updated correctly.
    // * <C-i>       Jump forward to line 1.
    test((
        "#[|]#",
        "[<space><C-s>kduU<C-o><C-i>",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // In this case we 'redo' manually to ensure that the transactions are composing correctly.
    test((
        "#[|]#",
        "[<space>u[<space>u",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_extend_line() -> anyhow::Result<()> {
    // extend with line selected then count
    test((
        indoc! {"\
            #[l|]#orem
            ipsum
            dolor
            
            "},
        "x2x",
        indoc! {"\
            #[lorem
            ipsum
            dolor\n|]#
            
            "},
    ))
    .await?;

    // extend with count on partial selection
    test((
        indoc! {"\
            #[l|]#orem
            ipsum
            
            "},
        "2x",
        indoc! {"\
            #[lorem
            ipsum\n|]#
            
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_character_info() -> anyhow::Result<()> {
    // UTF-8, single byte
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some("ih<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""h" (U+0068) Dec 104 Hex 68"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // UTF-8, multi-byte
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some("ië<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""ë" (U+0065 U+0308) Hex 65 + cc 88"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // Multiple characters displayed as one, escaped characters
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some(":line<minus>ending crlf<ret>:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""\r\n" (U+000d U+000a) Hex 0d + 0a"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // Non-UTF-8
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some(":encoding ascii<ret>ih<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(r#""h" Dec 104 Hex 68"#, app.editor.get_status().unwrap().0);
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_char_backward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("#(x|)# #[x|]#", "c<space><backspace><esc>", "#[\n|]#")).await?;
    test((
        "#( |)##( |)#a#( |)#axx#[x|]#a",
        "li<backspace><esc>",
        "#(a|)##(|a)#xx#[|a]#",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_word_backward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("fo#[o|]#ba#(r|)#", "a<C-w><esc>", "#[\n|]#")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_word_forward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("fo#[o|]#b#(|ar)#", "i<A-d><esc>", "fo#[\n|]#")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_char_forward() -> anyhow::Result<()> {
    test((
        indoc! {"\
                #[abc|]#def
                #(abc|)#ef
                #(abc|)#f
                #(abc|)#
            "},
        "a<del><esc>",
        indoc! {"\
                #[abc|]#ef
                #(abc|)#f
                #(abc|)#
                #(abc|)#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_with_indent() -> anyhow::Result<()> {
    const INPUT: &str = indoc! { "
        #[f|]#n foo() {
            if let Some(_) = None {

            }
         
        }

        fn bar() {

        }
        "
    };

    // insert_at_line_start
    test((
        INPUT,
        ":lang rust<ret>%<A-s>I",
        indoc! { "
            #[f|]#n foo() {
                #(i|)#f let Some(_) = None {
                    #(\n|)#
                #(}|)#
            #( |)#
            #(}|)#
            #(\n|)#
            #(f|)#n bar() {
                #(\n|)#
            #(}|)#
            "
        },
    ))
    .await?;

    // insert_at_line_end
    test((
        INPUT,
        ":lang rust<ret>%<A-s>A",
        indoc! { "
            fn foo() {#[\n|]#
                if let Some(_) = None {#(\n|)#
                    #(\n|)#
                }#(\n|)#
             #(\n|)#
            }#(\n|)#
            #(\n|)#
            fn bar() {#(\n|)#
                #(\n|)#
            }#(\n|)#
            "
        },
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections() -> anyhow::Result<()> {
    // normal join
    test((
        indoc! {"\
            #[a|]#bc
            def
        "},
        "J",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    // join with empty line
    test((
        indoc! {"\
            #[a|]#bc

            def
        "},
        "JJ",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    // join with additional space in non-empty line
    test((
        indoc! {"\
            #[a|]#bc

                def
        "},
        "JJ",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections_space() -> anyhow::Result<()> {
    // join with empty lines panic
    test((
        indoc! {"\
            #[a

            b

            c

            d

            e|]#
        "},
        "<A-J>",
        indoc! {"\
            a#[ |]#b#( |)#c#( |)#d#( |)#e
        "},
    ))
    .await?;

    // normal join
    test((
        indoc! {"\
            #[a|]#bc
            def
        "},
        "<A-J>",
        indoc! {"\
            abc#[ |]#def
        "},
    ))
    .await?;

    // join with empty line
    test((
        indoc! {"\
            #[a|]#bc

            def
        "},
        "<A-J>",
        indoc! {"\
            #[a|]#bc
            def
        "},
    ))
    .await?;

    // join with additional space in non-empty line
    test((
        indoc! {"\
            #[a|]#bc

                def
        "},
        "<A-J><A-J>",
        indoc! {"\
            abc#[ |]#def
        "},
    ))
    .await?;

    // join with retained trailing spaces
    test((
        indoc! {"\
            #[aaa   

            bb  

            c |]#
        "},
        "<A-J>",
        indoc! {"\
            aaa   #[ |]#bb  #( |)#c 
        "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections_comment() -> anyhow::Result<()> {
    test((
        indoc! {"\
            /// #[a|]#bc
            /// def
        "},
        ":lang rust<ret>J",
        indoc! {"\
            /// #[a|]#bc def
        "},
    ))
    .await?;

    // Only join if the comment token matches the previous line.
    test((
        indoc! {"\
            #[| // a
            // b
            /// c
            /// d
            e
            /// f
            // g]#
        "},
        ":lang rust<ret>J",
        indoc! {"\
            #[| // a b /// c d e f // g]#
        "},
    ))
    .await?;

    test((
        "#[|\t// Join comments
\t// with indent]#",
        ":lang go<ret>J",
        "#[|\t// Join comments with indent]#",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_read_file() -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    let contents_to_read = "some contents";
    let output_file = helpers::temp_file_with_contents(contents_to_read)?;

    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_file(file.path(), None)
            .build()?,
        Some(&format!(":r {:?}<ret><esc>:w<ret>", output_file.path())),
        Some(&|app| {
            assert!(!app.editor.is_err(), "error: {:?}", app.editor.get_status());
        }),
        false,
    )
    .await?;

    let expected_contents = LineFeedHandling::Native.apply(contents_to_read);
    helpers::assert_file_has_content(&mut file, &expected_contents)?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_delete() -> anyhow::Result<()> {
    // Test `surround_delete` when head < anchor
    test(("(#[|  ]#)", "mdm", "#[|  ]#")).await?;
    test(("(#[|  ]#)", "md(", "#[|  ]#")).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_replace_ts() -> anyhow::Result<()> {
    const INPUT: &str = r#"\
fn foo() {
    if let Some(_) = None {
        testing!("f#[|o]#o)");
    }
}
"#;
    test((
        INPUT,
        ":lang rust<ret>mrm'",
        r#"\
fn foo() {
    if let Some(_) = None {
        testing!('f#[|o]#o)');
    }
}
"#,
    ))
    .await?;

    test((
        INPUT,
        ":lang rust<ret>3mrm[",
        r#"\
fn foo() {
    if let Some(_) = None [
        testing!("f#[|o]#o)");
    ]
}
"#,
    ))
    .await?;

    test((
        INPUT,
        ":lang rust<ret>2mrm{",
        r#"\
fn foo() {
    if let Some(_) = None {
        testing!{"f#[|o]#o)"};
    }
}
"#,
    ))
    .await?;

    test((
        indoc! {"\
            #[a
            b
            c
            d
            e|]#
            f
            "},
        "s\\n<ret>r,",
        "a#[,|]#b#(,|)#c#(,|)#d#(,|)#e\nf\n",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn macro_play_within_macro_record() -> anyhow::Result<()> {
    // <https://github.com/helix-editor/helix/issues/12697>
    //
    // * `"aQihello<esc>Q` record a macro to register 'a' which inserts "hello"
    // * `Q"aq<space>world<esc>Q` record a macro to the default macro register which plays the
    //   macro in register 'a' and then inserts " world"
    // * `%d` clear the buffer
    // * `q` replay the macro in the default macro register
    // * `i<ret>` add a newline at the end
    //
    // The inner macro in register 'a' should replay within the outer macro exactly once to insert
    // "hello world".
    test((
        indoc! {"\
            #[|]#
        "},
        r#""aQihello<esc>QQ"aqi<space>world<esc>Q%dqi<ret>"#,
        indoc! {"\
            hello world
            #[|]#"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn global_search_with_multibyte_chars() -> anyhow::Result<()> {
    // Assert that `helix_term::commands::global_search` handles multibyte characters correctly.
    test((
        indoc! {"\
            // Hello world!
            // #[|
            ]#
            "},
        // start global search
        " /«十分に長い マルチバイトキャラクター列» で検索<ret><esc>",
        indoc! {"\
            // Hello world!
            // #[|
            ]#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reflow() -> anyhow::Result<()> {
    test((
        "#[|This is a long line bla bla bla]#",
        ":reflow 5<ret>",
        "#[|This
is a
long
line
bla
bla
bla]#",
    ))
    .await?;

    test((
        "// #[|This is a really long comment that we want to break onto multiple lines.]#",
        ":lang rust<ret>:reflow 13<ret>",
        "// #[|This is a
// really
// long
// comment
// that we
// want to
// break onto
// multiple
// lines.]#",
    ))
    .await?;

    test((
        "#[\t// Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
\t// tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim
\t// veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea
\t// commodo consequat. Duis aute irure dolor in reprehenderit in voluptate
\t// velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint
\t// occaecat cupidatat non proident, sunt in culpa qui officia deserunt
\t// mollit anim id est laborum.
|]#",
        ":lang go<ret>:reflow 50<ret>",
        "#[\t// Lorem ipsum dolor sit amet, consectetur
\t// adipiscing elit, sed do eiusmod
\t// tempor incididunt ut labore et dolore
\t// magna aliqua. Ut enim ad minim
\t// veniam, quis nostrud exercitation
\t// ullamco laboris nisi ut aliquip ex ea
\t// commodo consequat. Duis aute irure
\t// dolor in reprehenderit in voluptate
\t// velit esse cillum dolore eu fugiat
\t// nulla pariatur. Excepteur sint
\t// occaecat cupidatat non proident, sunt
\t// in culpa qui officia deserunt
\t// mollit anim id est laborum.
|]#",
    ))
    .await?;

    test((
        "    // #[|This document has multiple lines that each need wrapping

    /// currently we wrap each line completely separately in order to preserve existing newlines.]#",
        ":lang rust<ret>:reflow 40<ret>",
        "    // #[|This document has multiple lines
    // that each need wrapping

    /// currently we wrap each line
    /// completely separately in order
    /// to preserve existing newlines.]#"
    ))
    .await?;

    test((
        "#[|Very-long-words-should-not-be-broken-by-hard-wrap]#",
        ":reflow 2<ret>",
        "#[|Very-long-words-should-not-be-broken-by-hard-wrap]#",
    ))
    .await?;

    test((
        "#[|Spaces             are           removed           when          wrapping]#",
        ":reflow 2<ret>",
        "#[|Spaces
are
removed
when
wrapping]#",
    ))
    .await?;

    test((
        "Test wrapping only part of the text
#[|wrapping should only modify the lines that are currently selected
]#",
        ":reflow 11<ret>",
        "Test wrapping only part of the text
#[|wrapping
should only
modify the
lines that
are
currently
selected
]#",
    ))
    .await?;

    test((
        "#[|  TestWrappingLongWordWithIndent]#",
        ":reflow 2<ret>",
        "#[|  TestWrappingLongWordWithIndent]#",
    ))
    .await?;

    test((
        "#[| TestWrappingLongWordWithWordBoundaryAtTheStartOfTheDocument]#",
        ":reflow 2<ret>",
        "#[| TestWrappingLongWordWithWordBoundaryAtTheStartOfTheDocument]#",
    ))
    .await?;

    Ok(())
}
