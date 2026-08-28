# Core ownership and string performance

Measured 2026-08-26 on an Apple M1 Pro with Rust 1.98.0 in the release profile.

| Operation | `String` / deep clone | Shared snapshot |
| --- | ---: | ---: |
| Long label clone | 28.29–28.32 ns | `SharedString`: 9.66–9.67 ns |
| Format `Count 42` | 25.05–26.02 ns | `shared_format!`: 5.75–5.77 ns |

Allocation probes over 1,000 iterations:

| Operation | Allocations | Allocated bytes |
| --- | ---: | ---: |
| `String::from("static-label")` | 1,000 | 12,000 B |
| `SharedString::new_static("static-label")` | 0 | 0 B |
| Long `String::clone` | 1,000 | 59,000 B |
| Long `SharedString::clone` | 0 | 0 B |
| Deep `ElementNode::clone` test tree | 2,000 | 832,000 B |
| `Rc<ElementNode>` snapshot clone | 0 | 0 B |

`SharedString` wraps `SmolStr`: static values borrow without copying, values up to 23 bytes are
stored inline, and long values share heap storage. The component host retains each rendered tree in
an `Rc<ElementNode>` because MVP UI state is main-thread-only; platform projection clones this handle
instead of the full tree. Reproduce with:

```sh
cargo bench -p anmixiu-core --bench shared_string -- --noplot
cargo bench -p anmixiu-core --bench allocation
```
