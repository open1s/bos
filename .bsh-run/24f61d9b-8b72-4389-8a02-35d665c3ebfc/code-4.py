import os
os.environ["BSH_TOOL_SOCKET"] = "/Users/gaosg/Projects/bos/.bsh-run/24f61d9b-8b72-4389-8a02-35d665c3ebfc/tools-4.sock"
import bsh as _bsh
globals()["bsh"] = _bsh
import subprocess, os
os.chdir('/Users/gaosg/Projects/bos')
# Run workspace tests, capture only summary lines. Long timeout.
r = subprocess.run(['bash','-lc','cargo test --workspace 2>&1 | grep -E "^(test result|error|warning: unused|failures:)" | sort | uniq -c | sort -rn | head -20; echo "CARGO_EXIT:${PIPESTATUS[0]}"'], capture_output=True, text=True, timeout=590)
print(r.stdout[-3000:])
print("STDERR:", r.stderr[:500])