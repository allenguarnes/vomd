# Sections and HTML tables

<details>
<summary>A closed section, with <code>code</code> and a <a href="#tables">link</a></summary>

Its body shows once the section is opened.

- an item
- another item

</details>

<details open>
<summary><b>An open section</b></summary>

Its body shows from the start.

<details>
<summary>A section in it, closed</summary>

The nested section's body.

</details>

</details>

<details>

No summary: GitHub gives it one.

</details>

Text after the sections.

## Tables

<table>
<thead>
<tr><th>Name</th><th align="right">Score</th><th>Notes</th></tr>
</thead>
<tbody>
<tr><td>alpha</td><td align="right">1</td><td rowspan="2">spans two rows</td></tr>
<tr><td>beta</td><td align="right">22</td></tr>
<tr><td colspan="2">a cell across two columns</td><td>one line</td></tr>
<tr><td valign="top">at the top</td><td align="right" valign="bottom">333</td><td>a cell<br>of three<br>lines</td></tr>
</tbody>
</table>

<table>
<tr>
<td align="center"><a href="https://example.com"><img src="images/alpha.png" width="64px;" alt="Alpha"/><br /><sub><b>Alpha</b></sub></a></td>
<td align="center"><img src="images/alpha.png" width="64" alt="Beta"><br><sub><b>Beta Person</b></sub></td>
<td align="center"><img src="images/alpha.png" width="64" alt="Gamma"><br><sub><b>Gamma</b></sub></td>
</tr>
</table>

<table align="center">
<tr><th>Centered</th><th>table</th></tr>
<tr><td>

**Markdown** in a cell, after a blank line:

- a list
- in a cell

</td><td>plain</td></tr>
</table>

| Markdown | header | cells |
|---|:--|--:|
| centered | left | right |
| a cell long enough to wrap onto a second line in this narrow column | middle | middle |
