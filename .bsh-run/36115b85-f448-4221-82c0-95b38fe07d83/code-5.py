import os
os.environ["BSH_TOOL_SOCKET"] = "/Users/gaosg/Projects/bos/.bsh-run/36115b85-f448-4221-82c0-95b38fe07d83/tools-5.sock"
import bsh as _bsh
globals()["bsh"] = _bsh
print(bsh.bash("grep -rn '4\\.1\\.21' -r . --include=* 2>/dev/null | grep -v node_modules | grep -v 'target/' | grep -v '.bsh-run' | head -40"))
