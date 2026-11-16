// Turns the LCOV file of `node --test --test-reporter=lcov` into a Markdown
// table (lines, branches, functions per file) for $GITHUB_STEP_SUMMARY
// (#322). No npm, no dependencies.
//
//   node .github/scripts/coverage-summary.mjs frontend.lcov
import { readFileSync } from 'node:fs';
import { relative } from 'node:path';

const file = process.argv[2];
if (!file) {
    console.error('usage: coverage-summary.mjs <lcov file>');
    process.exit(2);
}

const records = [];
let current = null;
for (const line of readFileSync(file, 'utf8').split('\n')) {
    const [key, value] = line.split(/:(.*)/s);
    if (key === 'SF') current = { file: relative(process.cwd(), value), LF: 0, LH: 0, BRF: 0, BRH: 0, FNF: 0, FNH: 0 };
    else if (current && key in current && key !== 'file') current[key] = Number(value);
    else if (key === 'end_of_record' && current) { records.push(current); current = null; }
}
records.sort((a, b) => a.file.localeCompare(b.file));

const pct = (hit, found) => (found ? `${((100 * hit) / found).toFixed(1)} %` : '–');
const total = records.reduce((t, r) => {
    for (const k of ['LF', 'LH', 'BRF', 'BRH', 'FNF', 'FNH']) t[k] += r[k];
    return t;
}, { LF: 0, LH: 0, BRF: 0, BRH: 0, FNF: 0, FNH: 0 });

const row = (name, r) => `| ${name} | ${pct(r.LH, r.LF)} | ${pct(r.BRH, r.BRF)} | ${pct(r.FNH, r.FNF)} |`;
console.log('| File | Lines | Branches | Functions |');
console.log('|---|---:|---:|---:|');
for (const r of records) console.log(row(`\`${r.file}\``, r));
console.log(row('**All files**', total));
