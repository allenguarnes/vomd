# Highlighting

One short block for each language table, in GitHub's colours. The selection goldens start in
this sentence and end inside the Rust block.

```rust
/// Doc /* nested */ comment
#[derive(Debug)]
pub fn parse<'a>(s: &'a str) -> Option<u32> {
    let c = '\n'; let r = r#"raw "q" "#;
    println!("{} {}", 0x1F, 1.5e-3); // selected to here
    if s.len() > 2 { Some(42) } else { None }
}
```

```py
@dataclass
class Foo(Base):
    def bar(self, x=1.5):  # comment
        return f"hi {x}" + r'\d' if x else None
```

```ts
interface User { name: string; age?: number }
export async function load(id: number): Promise<User> {
  const res = await fetch(`/api/${id}`); // fetch
  return res.json() as User;
}
```

```json
{"name": "x", "n": [1, 2.5, -3e2], "ok": true, "z": null}
```

```yaml
# config
on: [push, "pull_request"]
jobs:
  build:
    - run: cargo test # c
      env: &e
        X: true
```

```toml
[package]
name = "vomd" # c
deps = ["a", 1, true]
```

```bash
#!/bin/bash
for f in *.md; do echo "file: $f ${HOME}" | grep -v '#x'; done # c
```

```console
$ cargo build --release
   Compiling vomd v1.7.0
user@host:~$ echo "hi" \
  && ls
hi
```

```c
#include <stdio.h>
int main(void) { printf("%d\n", 0xff); return 0; } /* done */
```

```go
package main
func main() { fmt.Println(`raw`, 42, nil) }
```

```java
@Override
public class Foo extends Bar { String s = "x"; } // c
```

```kotlin
data class User(val name: String)
fun greet(u: User) = print("hi ${u.name} $x") /* a /* b */ c */
```

```csharp
namespace App { class Foo { string p = @"C:\x"; var s = $"{p}"; } }
```

```swift
struct Point { var x = 0 }
func area(_ p: Point) -> Int { return p.x * 2 } // c
```

```html
<!-- c -->
<a href="x.html" class='b'>Hi &amp; bye</a><br/>
```

```css
a:hover, .btn #id { color: #fff; margin: 0 1.5em !important; }
@media (max-width: 600px) { body { font: 12px "Inter"; } }
```

```scss
$size: 4px;
.nav { ul { margin: $size; } } // c
```

```sql
SELECT id, COUNT(*) FROM users WHERE name = 'it''s' -- c
insert into t values (1, null);
```

```diff
@@ -1,2 +1,2 @@
-old
+new
 same
```

````markdown
# Title
- item `code` [link](http://x)
```rust
let x = 1;
```
````

```dockerfile
FROM rust:1.89 AS build
RUN cargo build --release # build
ENV PATH=$HOME/bin
```

```makefile
CC := gcc
build: main.c # c
	$(CC) -o app main.c
```

```ruby
class Foo < Bar
  def hi(x) = puts "hi #{x}" unless x.nil? # c
  @count = @@total + :sym.size
end
```

```php
<?php
function hi($name) { echo "hi $name"; } // c
```

```lua
local function f(t) return t[1] .. [[long string]] end -- c
```

```powershell
function Get-X { param($Name) if ($Name -eq $null) { Write-Host "x" } } # c
```

Unlabelled and `text` blocks stay plain:

```text
fn main() { "not highlighted" }
```
