# Inline HTML, autolinks and edge cases

Keys <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>P</kbd>, water is H<sub>2</sub>O, E = mc<sup>2</sup>,
<b>bold tag</b>, <i>italic tag</i>, <del>deleted</del>, and an <a href="https://example.com">HTML link</a>.

Bare links become links: www.commonmark.org/help, https://github.com/user/repo/issues?q=is%3Aopen,
(https://example.com/a_(b)) and mail@example.com. Code keeps them as text: `https://example.com`.

<script>this never shows</script><style>p { color: red }</style>

<table><tr><th>Name</th><th>Value</th></tr><tr><td>alpha</td><td>1</td></tr></table>

Escapes: \*not emphasis\*, \`not code\`, a\
hard break and two-space hard break  
next line.

***Bold italic***, **bold with *nested italic* inside**, ~~strike with **bold**~~,
`code with **stars**`, and a footnote.[^n]

> [!NOTE]
> Alert with `code`, a [link](#inline-html-autolinks-and-edge-cases) and **bold**.

> [!CAUTION]
> Second alert.

[^n]: The note, with a link to www.example.org.
