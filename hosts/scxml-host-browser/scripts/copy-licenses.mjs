// `npm pack` hook (`prepack`): copies the repository's license texts into
// the package root so the packed tarball carries the terms its
// `Apache-2.0 OR MIT` license field names. The copies are build output
// (ignored by git); the repository root files stay the only source.
import { copyFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspace = resolve(packageRoot, '../..');

const LICENSE_FILES = ['LICENSE-APACHE', 'LICENSE-MIT'];

for (const file of LICENSE_FILES) {
  await copyFile(join(workspace, file), join(packageRoot, file));
}
