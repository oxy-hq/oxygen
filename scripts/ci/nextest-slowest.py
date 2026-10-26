#!/usr/bin/env python3
"""Print the slowest tests in a nextest JUnit report as a Markdown table.

Usage: nextest-slowest.py <junit.xml> [count]

CI appends the output to the job summary of each test partition, so the slow
tail is visible on every run instead of being reconstructed from logs.
"""

import sys
import xml.etree.ElementTree as ET


def main() -> int:
    path = sys.argv[1]
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 15
    cases = [
        (float(case.get("time", "0")), case.get("classname", ""), case.get("name", ""))
        for case in ET.parse(path).getroot().iter("testcase")
    ]
    cases.sort(reverse=True)
    total = sum(seconds for seconds, _, _ in cases)
    print(f"### Slowest {min(count, len(cases))} of {len(cases)} tests ({total:.0f}s of test time)\n")
    print("| seconds | binary | test |")
    print("| ---: | --- | --- |")
    for seconds, binary, name in cases[:count]:
        print(f"| {seconds:.1f} | `{binary}` | `{name}` |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
