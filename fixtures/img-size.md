# Image sizes

HTML `<img>` with `width` and `height`; the same images as Markdown follow, at their own size.

<img src="images/gradient.jpg" width="128"> width="128"

<img src="images/gradient.jpg" width="128px"> width="128px"

<img src="images/gradient.jpg" height="40"> height="40"

<img src="images/gradient.jpg" width="64" height="64"> width="64" height="64" (stretched)

<img src="images/badge.svg" width="200" height="60"> an SVG in width="200" height="60" (fitted)

<img src="images/wide.jpg" width="50%"> width="50%"

<img src="images/wide.jpg" width="4000"> width="4000" (fitted to the column)

Ignored: <img src="images/gradient.jpg" width="0"> width="0",
<img src="images/gradient.jpg" width="abc"> width="abc",
<img src="images/gradient.jpg" width="12abc"> width="12abc",
<img src="images/gradient.jpg" width="2em"> width="2em".

| Half of a cell | Text |
|---|---|
| <img src="images/wide.jpg" width="50%"> | The image is half its cell. |

A remote one with both sides has its final box while it loads:
<img src="https://upload.wikimedia.org/wikipedia/commons/4/47/PNG_transparency_demonstration_1.png" width="160" height="120">

The README's icon line: <img src="../assets/icon@2x.png" width="128">

As Markdown: ![gradient](images/gradient.jpg) ![plasma](images/wide.jpg)
![icon](../assets/icon@2x.png)
