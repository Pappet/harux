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
    insertBefore(c, ref) {
        c.remove();
        c.parent = this;
        const i = ref ? this.children.indexOf(ref) : this.children.length;
        this.children.splice(i, 0, c);
        return c;
    }
    querySelectorAll(sel) {
        const cls = sel.replace(/^\./, '');
        return this.children.filter((c) => c.classList.contains(cls));
    }
    querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}
