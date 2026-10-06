# W3C SCXML IRP test-suite license notice

The upstream SCXML Implementation Report states that this test suite is
distributed under both the W3C Test Suite License and the W3C 3-clause BSD
License:

- https://www.w3.org/copyright/test-suite-license-2008/
- https://www.w3.org/copyright/3-clause-bsd-license-2008/
- Original suite: https://www.w3.org/Voice/2013/scxml-irp/

Copyright © 2015 World Wide Web Consortium (MIT, ERCIM, Keio, Beihang) and
others. All Rights Reserved.

## W3C 3-clause BSD terms

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the original copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the original copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.
3. Neither the name of W3C nor the names of its contributors may be used to
   endorse or promote products derived from this work without specific prior
   written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS “AS IS”
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

The unmodified upstream sources and W3C stylesheets in this directory may also
be used under the W3C Test Suite License linked above. Generated transformed
artifacts are retained for software development and reproducibility under the
3-clause BSD option; they are not represented as unmodified W3C tests.

The `scxml-conformance` crate compiles in the W3C manual instructions its
manual-equivalent oracle quotes verbatim from the upstream `.txml` sources,
and its tests embed upstream files from this directory, so its Cargo license
metadata is `(Apache-2.0 OR MIT) AND LicenseRef-W3C-Test-Suite`, where
`LicenseRef-W3C-Test-Suite` names the terms above. Neither W3C text matches a
license that has an SPDX identifier.
