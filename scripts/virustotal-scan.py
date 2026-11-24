#!/usr/bin/env python3
"""Scans a runtime build on VirusTotal and writes the result as JSON.

Usage: virustotal-scan.py DIR OUT_JSON --repo OWNER/NAME --commit SHA

Every .exe and .dll directly in DIR is looked up by SHA-256. A file VirusTotal has not
seen is uploaded (these are builds of public source code, made by this workflow) and
the analysis is waited for. The key comes from VT_API_KEY; without it the result says
"skipped". A free key allows 4 requests a minute, so requests are spaced out.

The JSON names the repository and commit the build came from, so the result stays tied
to that exact build:

  {"repo", "commit", "scanned_at", "verdict": "clean" | "flagged" | "pending" | "skipped" | "error",
   "files": [{"name", "sha256", "malicious", "suspicious", "harmless", "undetected", "engines", "link", "state"}]}

The exit code is 0 whatever the verdict; the caller decides what a verdict means.
"""

import argparse
import datetime
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid

API = os.environ.get("VT_API", "https://www.virustotal.com/api/v3")
GAP = float(os.environ.get("VT_GAP_SECONDS", "16"))        # 4 requests a minute
WAIT = float(os.environ.get("VT_WAIT_SECONDS", "900"))      # how long to wait for an analysis
DIRECT_UPLOAD = 32 << 20

_last = 0.0


def request(key, method, url, body=None, headers=None):
    """Returns (status, parsed JSON or None)."""
    global _last
    pause = _last + GAP - time.monotonic()
    if pause > 0:
        time.sleep(pause)
    req = urllib.request.Request(url, data=body, method=method, headers={"x-apikey": key, **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            status, data = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, data = e.code, e.read()
    finally:
        _last = time.monotonic()
    try:
        return status, json.loads(data) if data else None
    except ValueError:
        return status, None


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def stats_of(report):
    return ((report or {}).get("data") or {}).get("attributes", {}).get("last_analysis_stats") or {}


def upload(key, path):
    size = os.path.getsize(path)
    url = f"{API}/files"
    if size > DIRECT_UPLOAD:
        status, j = request(key, "GET", f"{API}/files/upload_url")
        if status != 200 or not j or not j.get("data"):
            raise RuntimeError(f"upload_url: HTTP {status}")
        url = j["data"]
    boundary = uuid.uuid4().hex
    with open(path, "rb") as f:
        data = f.read()
    name = os.path.basename(path).replace('"', "")
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n"
            f"Content-Type: application/octet-stream\r\n\r\n").encode() + data + f"\r\n--{boundary}--\r\n".encode()
    status, j = request(key, "POST", url, body, {"Content-Type": f"multipart/form-data; boundary={boundary}"})
    if status != 200 or not j:
        raise RuntimeError(f"upload: HTTP {status}")
    return j["data"]["id"]


def wait_for(key, analysis):
    deadline = time.monotonic() + WAIT
    while time.monotonic() < deadline:
        status, j = request(key, "GET", f"{API}/analyses/{analysis}")
        if status == 200 and ((j or {}).get("data") or {}).get("attributes", {}).get("status") == "completed":
            return True
        time.sleep(max(0.0, 30 - GAP))
    return False


def scan_file(key, path):
    digest = sha256(path)
    entry = {"name": os.path.basename(path), "sha256": digest, "link": f"https://www.virustotal.com/gui/file/{digest}"}
    status, report = request(key, "GET", f"{API}/files/{digest}")
    if status == 404:
        analysis = upload(key, path)
        print(f"{entry['name']}: uploaded, analysis {analysis}", file=sys.stderr)
        if wait_for(key, analysis):
            status, report = request(key, "GET", f"{API}/files/{digest}")
    if status not in (200, 404):
        raise RuntimeError(f"{entry['name']}: HTTP {status}")
    s = stats_of(report if status == 200 else None)
    for k in ("malicious", "suspicious", "harmless", "undetected"):
        entry[k] = int(s.get(k, 0))
    entry["engines"] = sum(entry[k] for k in ("malicious", "suspicious", "harmless", "undetected"))
    entry["state"] = "flagged" if entry["malicious"] else ("clean" if entry["engines"] else "pending")
    return entry


def scan(directory, key):
    names = sorted(n for n in os.listdir(directory)
                   if n.lower().endswith((".exe", ".dll")) and os.path.isfile(os.path.join(directory, n)))
    files = [scan_file(key, os.path.join(directory, n)) for n in names]
    states = {f["state"] for f in files}
    verdict = "flagged" if "flagged" in states else ("pending" if "pending" in states or not files else "clean")
    return verdict, files


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("out")
    ap.add_argument("--repo", required=True)
    ap.add_argument("--commit", required=True)
    a = ap.parse_args()
    result = {"repo": a.repo, "commit": a.commit,
              "scanned_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
    key = os.environ.get("VT_API_KEY", "").strip()
    if not key:
        result.update(verdict="skipped", files=[], note="no VT_API_KEY secret")
    else:
        try:
            result["verdict"], result["files"] = scan(a.dir, key)
        except Exception as e:  # the build is published as unchecked rather than not at all
            result.update(verdict="error", files=[], note=str(e))
    with open(a.out, "w") as f:
        json.dump(result, f, indent=2)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
