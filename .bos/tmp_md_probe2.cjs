// Probe: load the real app.js and exercise renderMarkdown on adversarial inputs.
const fs = require("fs");
const src = fs.readFileSync("crates/gui/ui/app.js", "utf8");

function grab(name) {
  const i = src.indexOf("function " + name);
  if (i < 0) throw new Error(name + " not found");
  let depth = 0, j = src.indexOf("{", i);
  for (let k = j; k < src.length; k++) {
    if (src[k] === "{") depth++;
    else if (src[k] === "}") { depth--; if (!depth) return src.slice(i, k + 1); }
  }
}
// MD_* constants
global.MD_BLOCK = String.fromCharCode(1);
global.MD_CODE = String.fromCharCode(2);
for (const n of ["escapeMd", "inlineMd", "codeBlockHtml", "renderMarkdown"]) eval(grab(n));

const cases = [
  "[a>b](https://x.example/)",
  "<img src=x onerror=alert(1)>",
  "```js\nconsole.log(1)",                 // unterminated fence
  "see `a<b` and **bold**",
  "[x](https://a.example/)" + '" onmouseover="alert(1)',
  '[" onclick="alert(1)](https://a.example/x)',
  "```" + "js\n" + "</script><script>alert(1)</script>\n```",
];
for (const c of cases) {
  console.log("IN :", JSON.stringify(c));
  console.log("OUT:", renderMarkdown(c));
  console.log("---");
}
