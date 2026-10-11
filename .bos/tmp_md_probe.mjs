// Probe: renderMarkdown inline pipeline on adversarial inputs.
const MD_CODE = String.fromCharCode(1);

function escapeMd(s) {
  let out = "";
  for (const ch of String(s)) {
    const c = ch.codePointAt(0);
    if (c >= 32 || ch === "\n" || ch === "\t") out += ch;
  }
  return out
    .split("&").join("&")
    .split("<").join("<")
    .split(String.fromCharCode(34)).join("&#34;");
}

function inlineMd(src) {
  const codes = [];
  let t = String(src)
    .split(">").join(">")
    .replace(/`([^`\n]+)`/g, (_, code) => {
      codes.push(code);
      return MD_CODE + (codes.length - 1) + MD_CODE;
    });
  t = t.replace(/\*\*([^\n]+?)\*\*/g, "<b>$1</b>");
  t = t.replace(/(^|[^*\w])\*([^*\n]+)\*/g, "$1<i>$2</i>");
  t = t.replace(/~~([^~\n]+)~~/g, "<s>$1</s>");
  t = t.replace(
    /(^|[^!])\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '$1<a href="$3" rel="noreferrer">$2</a>'
  );
  return t.replace(
    new RegExp(MD_CODE + "(\\d+)" + MD_CODE, "g"),
    (_, i) => "<code>" + codes[+i] + "</code>"
  );
}

const cases = [
  '[" onclick="alert(1)](https://a.example/x)',
  '[x](https://a.example/)" onmouseover="alert(1)',
  '[x](javascript:alert(1))',
  '[x](https://a.example/ style="position:fixed;top:0;left:0;width:100%;height:100%")',
];
for (const c of cases) {
  console.log("IN :", c);
  console.log("OUT:", inlineMd(escapeMd(c)));
  console.log("---");
}
