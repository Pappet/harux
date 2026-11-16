// Loads the frontend's plain <script> files (static/*.js) for the node:vm
// tests. Each file runs in the context under its own file name, so
// `node --test --experimental-test-coverage` reports static/*.js instead of
// only the test files (#322).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const staticUrl = (file) => new URL(`../../static/${file}`, import.meta.url);

// The source of `static/<file>`, for tests that read it as text.
export function staticSource(file) {
    return readFileSync(staticUrl(file), 'utf8');
}

// Runs `static/<file>` in the context `ctx`, as a <script> include would.
export function loadStatic(ctx, file) {
    const url = staticUrl(file);
    vm.runInContext(readFileSync(url, 'utf8'), ctx, { filename: fileURLToPath(url) });
}
