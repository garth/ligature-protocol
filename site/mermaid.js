// Renders the ```mermaid fences of the Ligature Protocol site (LP-181). mdBook emits
// each as <pre><code class="language-mermaid">; this swaps them for Mermaid's own
// element and draws them, in Mermaid's dark theme under mdBook's dark themes. Mermaid
// is loaded from the CDN, pinned, as the Material build loaded it before.
(() => {
    const blocks = document.querySelectorAll("code.language-mermaid");
    if (blocks.length === 0) {
        return;
    }
    const dark = /\b(coal|navy|ayu)\b/.test(document.documentElement.className);
    import("https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.esm.min.mjs")
        .then(({ default: mermaid }) => {
            for (const code of blocks) {
                const diagram = document.createElement("pre");
                diagram.className = "mermaid";
                diagram.textContent = code.textContent;
                code.parentElement.replaceWith(diagram);
            }
            mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
            return mermaid.run({ querySelector: "pre.mermaid" });
        })
        .catch((e) => console.error("mermaid:", e));
})();
