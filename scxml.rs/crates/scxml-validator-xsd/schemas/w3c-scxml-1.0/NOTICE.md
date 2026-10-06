# W3C SCXML 1.0 XML Schema — provenance and license notice

The `.xsd` files in this directory are the unmodified official XML Schema set
for State Chart XML (SCXML) 1.0, published by the W3C at the schema location
named by the SCXML 1.0 Recommendation (W3C Recommendation, 1 September 2015,
<https://www.w3.org/TR/2015/REC-scxml-20150901/>):

- Upstream directory: <https://www.w3.org/2011/04/SCXML/>
- Driver schema: <https://www.w3.org/2011/04/SCXML/scxml.xsd>
- Upstream `Last-Modified` of the driver when vendored: Tue, 04 Dec 2012 21:17:21 GMT
- Retrieved: 2026-09-23 over HTTPS; byte identity is pinned by the SHA-256
  digests below and re-verified by `scxml-validator-xsd` unit tests.

| File | Upstream URL | SHA-256 |
| --- | --- | --- |
| `scxml.xsd` | <https://www.w3.org/2011/04/SCXML/scxml.xsd> | `98e14d0e03d5f9bb49b03fe4a2c24d72c165789d68fe93eee24fc403e4b23b48` |
| `scxml-module-core.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-module-core.xsd> | `cbe0cde7a2cc165b84263e0dad45abd74296ed99a9196611e4b0c98b494222e4` |
| `scxml-module-data.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-module-data.xsd> | `c34438431aa5d9815a63a64415e3127b832abd451ca509ff79a89d529dddba47` |
| `scxml-module-external.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-module-external.xsd> | `899b13488332a6b37700ea53766e16d39b1b4f38fb829d0d3b12f0955a08057d` |
| `scxml-datatypes.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-datatypes.xsd> | `51036cac697db9be6f92ad853215dc3166990ca292786fed1612d0666932163b` |
| `scxml-attribs.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-attribs.xsd> | `b74de05e9e8d86123a4e8cca6655761fae78009e8d63b95cc27ae38222a2de01` |
| `scxml-contentmodels.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-contentmodels.xsd> | `704baaf6e19c480fa76039390dc0a0e045f5544b5f68ec294c47fe51d40d0053` |
| `scxml-copyright.xsd` | <https://www.w3.org/2011/04/SCXML/scxml-copyright.xsd> | `a980a440bf960a57168230c5bcd60f2cfe82e1dc287e339850b2ef42cc22e35b` |

`scxml-copyright.xsd` declares no schema components; it is vendored because it
carries the license text referenced by every other module. The driver imports
the XML namespace schema from `http://www.w3.org/2001/xml.xsd`; that import is
not vendored or fetched because no SCXML 1.0 module references an `xml:*`
attribute or component, and the validator installs no network resolver.

## License

The schema modules carry the following W3C notice (from `scxml-copyright.xsd`),
which permits redistribution with this notice:

> This is the XML Schema for SCXML 1.0, formulated as a modular XML application
> Copyright ©1998-2007 World Wide Web Consortium
> (Massachusetts Institute of Technology, European Research Consortium
> for Informatics and Mathematics, Keio University).
> All Rights Reserved.
>
> Permission to use, copy, modify and distribute the SCXML Schema
> modules and their accompanying xs:documentation for any purpose
> and without fee is hereby granted in perpetuity, provided that the above
> copyright notice and this paragraph appear in all copies.
> The copyright holders make no representation about the suitability of
> these XML Schema modules for any purpose.
>
> They are provided "as is" without expressed or implied warranty.

These files are third-party material; they are not covered by this
repository's Apache-2.0 OR MIT license. Byte identity is recorded for
provenance and is not itself a license grant. Because `scxml-validator-xsd`
compiles these files in, its Cargo license metadata is
`(Apache-2.0 OR MIT) AND LicenseRef-W3C-SCXML-Schema`, where
`LicenseRef-W3C-SCXML-Schema` names the notice above; it matches none of the
W3C license texts that have an SPDX identifier.
