// Extension packages for the tests: an ExtensionPackage (manifest fields and
// schema bytes) read from a workspace manifest and its schema artifact, as
// `scx` reads `--extension-package MANIFEST.json`.
import { readFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const workspace = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');

/** The ExtensionPackage of the workspace-relative manifest `manifestPath`. */
export async function readPackage(manifestPath) {
  const manifest = JSON.parse(await readFile(join(workspace, manifestPath), 'utf8'));
  return {
    namespaceUri: manifest['namespace-uri'],
    packageVersion: manifest['package-version'],
    extensionArtifact: manifest['extension-artifact'],
    schemaArtifact: manifest['schema-artifact'],
    schemaSha256: manifest['schema-sha256'],
    witWorld: manifest['wit-world'],
    witVersion: manifest['wit-version'],
    schema: new Uint8Array(await readFile(join(workspace, dirname(manifestPath), manifest['schema-artifact']))),
  };
}
