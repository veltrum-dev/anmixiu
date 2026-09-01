# Legal and distribution notes

This is an operational compliance guide, not legal advice or an intellectual-property warranty.
The rules that apply to a particular distributor depend on its product, build, jurisdiction, and
distribution channel.

## Anmixiu license

Anmixiu is licensed under MIT. The license permits commercial, proprietary, and open-source use,
modification, and redistribution. A distributor must keep the complete Anmixiu copyright and MIT
license notice with copies or substantial portions of the software. Each published Anmixiu crate
is configured to include the repository's `LICENSE` and `README.md` in its source package.

The MIT text disclaims warranties but does not waive mandatory consumer, product, privacy,
accessibility, export, or other regulatory duties. It also contains no express patent-license
clause, trademark license, or intellectual-property indemnity.

## Third-party dependencies

Cargo dependencies retain their own licenses; Anmixiu's MIT license does not replace them. The
normal dependency graph in `Cargo.lock` currently uses permissive license expressions and contains
no dependency declared solely under GPL, AGPL, LGPL, MPL, SSPL, or another source-disclosure
license. This statement is only a snapshot: target selection, features, dependency updates, and an
application's additional dependencies can change the result.

Before every release, generate notices from the exact locked dependency graph for every shipped
target and feature set. Keep each dependency's copyright notice and selected license text with the
source or binary distribution. In the current macOS graph, pay particular attention to `slotmap`'s
Zlib license and `unicode-ident`'s mandatory Unicode-3.0 terms in addition to the selected MIT terms.
Development and benchmark dependencies are not part of a downstream application's normal link
graph, but their terms still apply when their source or binaries are redistributed.

Useful release checks include:

```sh
cargo metadata --locked --format-version 1
cargo tree -p anmixiu --edges normal --all-features --target aarch64-apple-darwin \
  --format '{p}\t{l}'
cargo package -p anmixiu --list
```

Use a license-notice generator such as `cargo-about` for a shipped application. Review generated
output instead of treating an automated SPDX scan as a legal conclusion.

## Platform software, fonts, and distribution channels

The repository calls public macOS AppKit, CoreText, Metal, CoreGraphics, QuartzCore, and Grand
Central Dispatch APIs through Rust dependencies. It does not bundle Apple system frameworks or
system font files. Developers who build or distribute Apple-platform applications remain
responsible for the terms covering Xcode and Apple SDKs, code signing and notarization, App Store
rules where applicable, entitlements, privacy disclosures, and any use of Apple names or marks.

Applications that select a named font must separately confirm that the font is installed or that
their intended embedding and redistribution are licensed. Referring to a system font family is not
the same as obtaining permission to ship the font file.

## Accessibility and regulated products

The current MVP does not provide an accessibility tree or assistive-technology integration. A
downstream application should not represent itself as accessible merely because it uses Anmixiu.
Products and services subject to accessibility law, public-sector procurement rules, medical or
safety regulation, financial regulation, or similar requirements need a product-specific review
and may need functionality outside the present framework.

## Names, patents, and provenance

The software license does not grant rights to the `Anmixiu` name or to third-party names. A project
name search, domain search, or registry search is not a trademark clearance; maintainers planning a
commercial launch should obtain jurisdiction- and class-specific clearance.

`Anmixiu` is also the direct pinyin spelling of “安迷修”, a character name used in the commercially
exploited *Aotu World* / 《凹凸世界》 franchise. That fact does not by itself establish trademark
infringement in software, but it creates a material association and confusion risk. Before building
commercial goodwill in this name, search exact, similar, Chinese-character, and transliterated marks
in the relevant software and technology classes in every target market and consider a different
name if clearance is uncertain.

No repository scan can prove freedom to operate under every software or user-interface patent.
Commercial adopters with material exposure should evaluate patent risk for their actual product and
markets. Contributors must follow the repository's source-provenance policy and disclose any code
derived from third-party material so its license can be reviewed before release.
