import os
os.environ["BSH_TOOL_SOCKET"] = "/Users/gaosg/Projects/bos/.bsh-run/59f17fcd-db6c-4dd3-ad46-23cfffcc680b/tools-334.sock"
import bsh as _bsh
globals()["bsh"] = _bsh
import re
src=open("crates/bsh/src/runtime/contract.rs").read()
for name in ["TaskReport","TaskSectionView","TaskRecordView","TaskRunView","TaskGraphView"]:
    m=re.search(r"(/// .*\n)*pub struct %s \{.*?\n\}"%name, src, re.S)
    print("=== ",name, "found" if m else "missing")
    if m: print(m.group(0)[:2500])
