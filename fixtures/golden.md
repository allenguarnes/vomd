# Golden render

Every block kind the layout draws, for the golden images. Paragraph text with
**bold**, *italic*, ***both***, `inline code`, ~~strikethrough~~, a [link](https://example.com),
a footnote reference[^note], H~2~O and x^2^ where the parser allows, and an ![image alt](missing.png).
A hard line break follows\
right here.

## Second-level heading

### Third

#### Fourth

##### Fifth

###### Sixth

---

- Bullet one
- Bullet two with enough words that it has to wrap onto a second line at the golden widths used here
  - Nested bullet
    - Third level
1. First
2. Second
   1. Nested ordered
10. Third (written as 10.: only the first number counts)

- [x] Done task
- [ ] Open task

> A block quote with *emphasis*.
>
> > A nested quote.

> [!NOTE]
> Useful information.

> [!TIP]
> A helpful hint.

> [!IMPORTANT]
> Key information.

> [!WARNING]
> Needs attention.

> [!CAUTION]
> Risky outcome.

```text
A code block.
    Indented line, and a long one that must wrap inside the block so the continuation marker shows up.
	Tab-indented line.
```

| Left | Center | Right |
|:-----|:------:|------:|
| a | `b` | 1.00 |
| longer cell content that wraps when the column is narrow | **c** | 22.50 |
| [link](#golden-render) | *d* | 333.25 |

Term with trailing text after the table.

[^note]: The footnote text, with `code`.
