# Bundled GUI fonts

Neutrasearch embeds these files so filenames and controls render consistently across Linux, macOS, and Windows without relying on platform defaults.

| Asset | Purpose | License | SHA-256 |
|---|---|---|---|
| `Inter-Variable.ttf` | Primary UI text (design system typeface; default weight instance) | SIL OFL-1.1 | `29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031` |
| `RobotoMono-Variable.ttf` | Paths, sizes, and counts (design system monospace; default weight instance) | SIL OFL-1.1 | `66a80e79d17e4c7cabd162e2916578a4cc08fd19eef6e2a643305eae9c567b2b` |
| `NotoSans-Regular.ttf` | Latin fallback behind Inter | Apache-2.0 | `478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823` |
| `NotoSansMono-Regular.ttf` | Latin fallback behind Roboto Mono | Apache-2.0 | `65b5e2b2c4a1fba9ae8be1f026cb35b03dcb8886d9b2a4147054fde12f7e767d` |
| `NotoSansArabic-Regular.ttf` | Arabic filename fallback | Apache-2.0 | `bdff3e5659d67e67def05b33f749683b9376ae819d65d3dd62ac4640b3aaef48` |
| `NotoSansDevanagari-Regular.ttf` | Devanagari filename fallback | Apache-2.0 | `306b53ecfb182a504dd8a7446093c316387d2fd8dc350d0792ed1753fe0996cd` |
| `NotoSansCJK-Regular.ttc` | CJK filename fallback | SIL OFL-1.1 | `b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a` |
| `NotoSansSymbols-Regular.ttf` | Symbol fallback | Apache-2.0 | `d0e98e9a2c046594c5021437273943be7e79e0fd980fde125279e22302212595` |
| `NotoSansSymbols2-Regular.ttf` | Extended symbol fallback | Apache-2.0 | `c4a0a80f0041ce4be81e2478faad22776d23edb98ae3f0d19bd37044820ecf9d` |

The files are unmodified. License texts are included beside them as `LICENSE-NOTO-APACHE-2.0.txt`, `LICENSE-NOTO-CJK-OFL-1.1.txt`, `LICENSE-INTER-OFL-1.1.txt`, and `LICENSE-ROBOTOMONO-OFL-1.1.txt`. Inter and Roboto Mono come from the google/fonts repository. Vector-painted application icons are used instead of depending on icon-font private-use glyphs.
