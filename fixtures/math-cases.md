# Math $e^{i\pi} + 1 = 0$ in a heading

Inline math sits on the baseline of the text: $E = mc^2$, $\alpha_i + \beta^2$, a fraction
$\frac{a}{b}$, a root $\sqrt{x^2 + y^2}$ and a sum $\sum_{k=1}^n k = \frac{n(n+1)}{2}$. A taller
one, $\dfrac{1}{1 + \frac{1}{x}}$, spreads its line apart; a [link with $x^2$](https://example.com)
stays a link. Prices like $5 and $10 stay text.

## Lists and quotes

- An item with $a^2 + b^2 = c^2$ in it
  $$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$$
- Another with $\lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n = e$

> A quote with $\vec{v} \cdot \vec{w}$ and a display formula:
>
> $$\nabla \times \vec{B} = \mu_0 \vec{J} + \mu_0 \varepsilon_0 \frac{\partial \vec{E}}{\partial t}$$

| Symbol | Meaning |
|---|---|
| $\pi$ | $3.14159\ldots$ |
| $\binom{n}{k}$ | $\frac{n!}{k!\,(n-k)!}$ |

## Display

$$
f(x) = \frac{1}{\sigma\sqrt{2\pi}} \exp\left(-\frac{(x-\mu)^2}{2\sigma^2}\right)
$$

$$
A = \begin{pmatrix} a_{11} & a_{12} & \cdots & a_{1n} \\ a_{21} & a_{22} & \cdots & a_{2n} \\ \vdots & \vdots & \ddots & \vdots \\ a_{m1} & a_{m2} & \cdots & a_{mn} \end{pmatrix}
\qquad \det\begin{vmatrix} a & b \\ c & d \end{vmatrix} = ad - bc
$$

$$
|x| = \begin{cases} x & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}
$$

```math
\begin{aligned}
(a + b)^2 &= (a + b)(a + b) \\
          &= a^2 + 2ab + b^2
\end{aligned}
```

$$
e^{i\theta} = \cos\theta + i \sin\theta \tag{1.1}
$$

A wide formula is broken into lines before its relations and operators, at full size:

$$
\sum_{n=0}^{\infty} \frac{f^{(n)}(a)}{n!}(x-a)^n = f(a) + f'(a)(x-a) + \frac{f''(a)}{2!}(x-a)^2 + \frac{f'''(a)}{3!}(x-a)^3 + \frac{f^{(4)}(a)}{4!}(x-a)^4 + \frac{f^{(5)}(a)}{5!}(x-a)^5 + \cdots
$$

one with nowhere to break is shrunk to fit:

$$
\left[\frac{f''(a)}{2!}(x-a)^2 \cdot \frac{f'''(a)}{3!}(x-a)^3 \cdot \frac{f^{(4)}(a)}{4!}(x-a)^4 \cdot \frac{f^{(5)}(a)}{5!}(x-a)^5 \cdot \frac{f^{(6)}(a)}{6!}(x-a)^6\right]
$$

and a much wider one goes on for several lines:

$$
(x_1 + x_2 + x_3 + x_4)^2 = x_1^2 + x_2^2 + x_3^2 + x_4^2 + 2x_1x_2 + 2x_1x_3 + 2x_1x_4 + 2x_2x_3 + 2x_2x_4 + 2x_3x_4 = \sum_{i=1}^4 x_i^2 + 2\sum_{1 \le i < j \le 4} x_i x_j = \left(\sum_{i=1}^4 x_i\right)^2
$$

An error shows its source: $\frac{1}$ and

$$
\undefinedcommand{x}
$$

Selected: the fraction $\frac{a+b}{c}$ keeps its bar over the wash, end of selection.
