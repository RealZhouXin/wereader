# Third-party notices

The rereader application as a whole is distributed under GPL-3.0-only;
see `LICENSE`. Third-party components retain their original copyright and
license notices. The source archive includes vendored Cargo dependencies
with their upstream notices and license files. The Kindle package also
includes dependency license and notice files in `licenses/`.

## Slint 1.18.1

Slint is used under its GNU GPL version 3 option (GPL-3.0-only), including
in the Kindle build. See `LICENSE` and
[the upstream licensing declaration](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSE.md).
Slint's royalty-free license is not the license selected by this project.

## slint-backend-kindle 0.7.1

[slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)
is offered under MIT OR Apache-2.0; this distribution selects MIT.
Its `LICENSE-MIT` is included in the Kindle package as
`slint-backend-kindle-LICENSE-MIT` and in the vendored source archive.

## SQLite storage and musl C headers

`rusqlite` 0.41.0 and `libsqlite3-sys` 0.39.1 retain their MIT licenses.
SQLite is compiled from the bundled source in `libsqlite3-sys`; its source
contains the upstream public-domain declaration. Cargo vendoring and the
license-copying package step include the crate sources and notices.

ARM builds use musl 1.2.5 headers from the pinned source tarball. The source
archive retains `.toolchains/musl-1.2.5.tar.gz` and `musl-COPYRIGHT`;
the binary package includes `licenses/musl-1.2.5/COPYRIGHT`.
The header setup script verifies the tarball SHA-256 before extraction.

## weread-omni

The E-ink protocol implementation in `src/weread/` is adapted from
[teng-lin/weread-omni](https://github.com/teng-lin/weread-omni),
commit `88bd2e095d7d7ee423eaadf8f40653e72c5be6d4`.
The upstream license follows.

MIT License

Copyright (c) 2026 Teng Lin

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Noto Sans SC

The bundled `assets/NotoSansSC-Regular.otf` is distributed unchanged from
[notofonts/noto-cjk](https://github.com/notofonts/noto-cjk). Its SIL Open Font
License 1.1 is included in `assets/NotoSansSC-LICENSE.txt` and in the Kindle package.
The font's copyright notices are retained in its metadata.
