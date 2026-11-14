// A minimal fake DOM element for the node:vm tests of static/*.js: just what
// the list code (removeRowsFromList, updateTruncationNote, buildMessageRow)
// uses. No jsdom, no npm.
export class El {
    constructor(className = '', id = '') {
        this.className = className;
        this.id = id;
        this.dataset = {};
        this.children = [];
        this.parent = null;
        this.style = {};
        this.textContent = '';
        const el = this;
        this.classList = {
            contains: (c) => el.className.split(' ').includes(c),
            add: (c) => { if (!el.classList.contains(c)) el.className = `${el.className} ${c}`.trim(); },
            remove: (c) => { el.className = el.className.split(' ').filter((x) => x !== c).join(' '); },
            toggle: (c, on) => (on ? el.classList.add(c) : el.classList.remove(c)),
        };
    }
    setAttribute(k, v) { (this.attrs ||= {})[k] = String(v); }
    getAttribute(k) { return (this.attrs || {})[k] ?? null; }
    removeAttribute(k) { if (this.attrs) delete this.attrs[k]; }
    get firstChild() { return this.children[0] || null; }
    cloneNode(deep) {
        const c = new El(this.className, this.id);
        c.tagName = this.tagName;
        Object.assign(c.dataset, this.dataset);
        c.attrs = { ...(this.attrs || {}) };
        c.textContent = this.textContent;
        c.title = this.title;
        c.tabIndex = this.tabIndex;
        c.style = { ...this.style };
        if (deep) for (const child of this.children) c.appendChild(child.cloneNode(true));
        return c;
    }
    get lastElementChild() { return this.children[this.children.length - 1] || null; }
    get firstElementChild() { return this.children[0] || null; }
    sibling(delta) {
        if (!this.parent) return null;
        const i = this.parent.children.indexOf(this);
        return this.parent.children[i + delta] || null;
    }
    get previousElementSibling() { return this.sibling(-1); }
    get nextElementSibling() { return this.sibling(1); }
    remove() {
        if (this.parent) this.parent.children.splice(this.parent.children.indexOf(this), 1);
        this.parent = null;
    }
    replaceChildren(...nodes) {
        for (const c of [...this.children]) c.remove();
        for (const n of nodes) this.appendChild(n);
    }
    appendChild(c) { c.remove(); c.parent = this; this.children.push(c); return c; }
    replaceChild(c, old) {
        const i = this.children.indexOf(old);
        if (i < 0) throw new Error('fake-dom: replaceChild of a node that is no child');
        c.remove();
        old.parent = null;
        c.parent = this;
        this.children[i] = c;
        return old;
    }
    insertBefore(c, ref) {
        c.remove();
        c.parent = this;
        const i = ref ? this.children.indexOf(ref) : this.children.length;
        this.children.splice(i, 0, c);
        return c;
    }
    // All descendants matching one compound selector: a tag, classes and
    // attributes — `.row`, `tr[data-label]`, `.field-input[data-field-path="PID-5"]`.
    // `data-*` attributes are read from `dataset`. Enough for the code under
    // test; no combinators.
    querySelectorAll(sel) {
        const match = matcher(sel);
        const out = [];
        const walk = (el) => {
            for (const c of el.children) {
                if (match(c)) out.push(c);
                walk(c);
            }
        };
        walk(this);
        return out;
    }
    querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
    contains(el) {
        for (let e = el; e; e = e.parent) if (e === this) return true;
        return false;
    }
}

function matcher(sel) {
    const m = /^([a-zA-Z]*)((?:\.[\w-]+)*)((?:\[[^\]]+\])*)$/.exec(sel.trim());
    if (!m) throw new Error(`fake-dom: unsupported selector ${sel}`);
    const tag = m[1].toUpperCase();
    const classes = m[2] ? m[2].split('.').filter(Boolean) : [];
    const attrs = [...m[3].matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)].map((a) => [a[1], a[2]]);
    const camel = (name) => name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    return (el) => {
        if (tag && String(el.tagName || '').toUpperCase() !== tag) return false;
        if (!classes.every((c) => el.classList.contains(c))) return false;
        return attrs.every(([name, value]) => {
            const v = name.startsWith('data-') ? el.dataset[camel(name)] : el.getAttribute(name);
            return value === undefined ? v != null : v === value;
        });
    };
}
