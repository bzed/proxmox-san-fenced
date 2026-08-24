import json
import os
import glob
import re

directory = 'new-tests'
files = glob.glob(os.path.join(directory, '*.json'))

for filepath in files:
    with open(filepath, 'r') as f:
        content = f.read()

    # Replace UUID
    content = re.sub(r'"uuid"\s*:\s*"[^"]+"', '"uuid" : "360000000000000000000000000000001"', content)

    # Replace WWNN and WWPN
    content = re.sub(r'"host_wwnn"\s*:\s*"[^"]+"', '"host_wwnn" : "0x5000000000000001"', content)
    content = re.sub(r'"target_wwnn"\s*:\s*"[^"]+"', '"target_wwnn" : "0x2000000000000001"', content)
    content = re.sub(r'"host_wwpn"\s*:\s*"[^"]+"', '"host_wwpn" : "0x5000000000000002"', content)
    content = re.sub(r'"target_wwpn"\s*:\s*"[^"]+"', '"target_wwpn" : "0x2000000000000002"', content)

    with open(filepath, 'w') as f:
        f.write(content)

print("Anonymization complete.")
