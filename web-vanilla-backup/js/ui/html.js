/** Tiny tagged-template helper shared by all view modules. */

/**
 * Builds a DocumentFragment from an HTML template string.
 * Interpolated values are escaped; fragments and arrays of nodes pass through.
 */
export function html(strings, ...values) {
  const template = document.createElement("template");
  let out = "";
  for (let i = 0; i < strings.length; i += 1) {
    out += strings[i];
    if (i < values.length) out += render(values[i]);
  }
  template.innerHTML = out;
  return template.content;
}

function render(value) {
  if (value == null) return "";
  if (Array.isArray(value)) return value.map(render).join("");
  if (value instanceof DocumentFragment) {
    const holder = document.createElement("div");
    holder.appendChild(value.cloneNode(true));
    return holder.innerHTML;
  }
  if (value instanceof Element) return value.outerHTML;
  return escape(String(value));
}

function escape(text) {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
