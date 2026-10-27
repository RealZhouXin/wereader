#!/usr/bin/env python3
"""Probe one chapter with the current local session; never print credentials/body.

Download inspection uses Python's standard library; --verify-plaintext also
requires cryptography. Does not refresh or change the session, upload reading
progress, purchase content, or write extracted text to disk.
"""

import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import sqlite3
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile
import xml.etree.ElementTree as ET

LIMIT = 16 * 1024 * 1024
BASE = "https://i.weread.qq.com"
HEADERS = {
    "baseapi": "30",
    "appver": "2.1.2.10245900",
    "basever": "2.1.2.10245900",
    "osver": "11",
    "channelid": "900",
    "User-Agent": "WeRead/2.1.2 WRBrand/Onyx wr_eink Dalvik/2.1.0 "
    "(Linux; U; Android 11; BOOX Build/onyx)",
}


class ProbeError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def data_dir():
    if os.environ.get("REREADER_DATA_DIR"):
        return Path(os.environ["REREADER_DATA_DIR"])
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "rereader"


def request(opener, headers, path, params=None, body=None):
    url = BASE + path
    if params:
        url += "?" + urllib.parse.urlencode(params)
    headers = dict(headers)
    payload = None
    if body is not None:
        payload = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    try:
        response = opener.open(
            urllib.request.Request(url, data=payload, headers=headers), timeout=30
        )
    except urllib.error.HTTPError as error:
        response = error
    except (OSError, urllib.error.URLError):
        raise ProbeError("Network/TLS request failed; no automatic retry.") from None
    with response:
        raw = response.read(LIMIT + 1)
        if len(raw) > LIMIT:
            raise ProbeError("Response exceeds 16 MiB limit.")
        meta = {
            "http": response.code,
            "content_type": response.headers.get("Content-Type", ""),
            "bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "encrypt_key_present": bool(response.headers.get("encryptKey")),
        }
    try:
        value = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        value = None
    if isinstance(value, dict):
        code = value.get("errcode", value.get("errCode"))
        # Never echo raw error messages, response headers, or arbitrary JSON.
        if isinstance(code, int):
            meta["error_code"] = code
    return meta, value, raw, response.headers.get("encryptKey")


def require_json(result):
    meta, value, *_ = result
    if meta["http"] != 200 or meta.get("error_code", 0) != 0 or not isinstance(value, dict):
        raise ProbeError("Metadata request rejected: " + json.dumps(meta))
    return value


def verify_plaintext(raw, envelope, vid, book_id, chapter_uid):
    """Verify the E-Ink 2.1.2 raw-VID envelope and chapter package in memory.

    Protocol reference: Zz2217967571/weread-kindle, direct/weread_direct.py,
    commit 2d9c8d6775786055f2955fbc1532a525f8067dd1, content_key().
    """
    from cryptography.hazmat.primitives import padding
    from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

    if not vid or not vid.isascii() or not vid.isdecimal() or not envelope:
        raise ProbeError("Missing or invalid key-envelope inputs.")
    try:
        encrypted = base64.b64decode(envelope, validate=True)
        if not encrypted or len(encrypted) % 16:
            raise ValueError()
        digits = vid.encode("ascii")
        material = (digits * ((32 + len(digits) - 1) // len(digits)))[:32]
        decryptor = Cipher(algorithms.AES(material[:16]), modes.CBC(material[16:])).decryptor()
        padded = decryptor.update(encrypted) + decryptor.finalize()
        unpadder = padding.PKCS7(128).unpadder()
        password = unpadder.update(padded) + unpadder.finalize()
        if not password:
            raise ValueError()
    except ValueError:
        raise ProbeError("Key-envelope decoding or PKCS#7 validation failed.") from None

    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        entries = archive.infolist()
        names = [x.filename for x in entries]
        if len(entries) > 512 or len(set(names)) != len(names) or sum(x.file_size for x in entries) > LIMIT:
            raise ProbeError("Unsafe or oversized chapter package.")
        files = {}
        for entry in entries:
            path = PurePosixPath(entry.filename)
            if path.is_absolute() or ".." in path.parts or "\\" in entry.filename or (entry.external_attr >> 16) & 0o170000 == 0o120000:
                raise ProbeError("Unsafe chapter resource path.")
            if entry.is_dir():
                continue
            try:
                with archive.open(entry, pwd=password) as resource:
                    data = resource.read(LIMIT + 1)
                if len(data) > LIMIT or len(data) != entry.file_size:
                    raise ProbeError("Chapter resource exceeds declared size.")
                files[entry.filename] = data
            except (RuntimeError, zipfile.BadZipFile, NotImplementedError):
                raise ProbeError("ZIP password, compression or CRC verification failed.") from None
    try:
        info = json.loads(files["info.txt"])
        if str(info.get("bookId")) != book_id:
            raise ValueError()
        chapters = [x for x in info["chapters"] if str(x.get("chapterUid")) == chapter_uid]
        if len(chapters) != 1:
            raise ValueError()
        resources = chapters[0]["files"]
        if not resources or not all(isinstance(x, str) and x in files for x in resources):
            raise ValueError()
        documents = []
        for name in resources:
            if name.lower().endswith((".xhtml", ".html", ".htm")):
                document = ET.fromstring(files[name])
                documents.append({"name": name, "bytes": len(files[name]),
                                  "xml_valid": True,
                                  "text_characters": sum(len(x) for x in document.itertext())})
        if not documents:
            raise ValueError()
    except (KeyError, ValueError, TypeError, AttributeError, ET.ParseError):
        raise ProbeError("Chapter identity, resource association or XHTML validation failed.") from None
    return {"algorithm": "raw-vid-aes128-cbc-pkcs7-zipcrypto",
            "book_matches": True, "chapter_matches": True,
            "crc_verified_entries": len(files), "documents": documents}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=data_dir())
    parser.add_argument("--book-id", required=True)
    parser.add_argument("--chapter-uid", help="Default: first free textual chapter.")
    parser.add_argument("--archive", type=Path, help="Save ZIP exclusively with mode 0600.")
    parser.add_argument("--auth-control", action="store_true", help="Repeat once without credentials.")
    parser.add_argument("--verify-plaintext", action="store_true", help="Verify decryption/CRC/identity/XHTML in memory; requires cryptography.")
    args = parser.parse_args()
    if args.verify_plaintext:
        try:
            import cryptography  # noqa: F401
        except ImportError:
            raise ProbeError("--verify-plaintext requires cryptography in this Python environment.") from None

    database = (args.data_dir / "rereader.sqlite3").resolve()
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        session = connection.execute(
            "SELECT account_id,access_token FROM sessions WHERE singleton=1"
        ).fetchone()
    if not session:
        raise ProbeError("No local session; sign in through the app first.")
    headers = dict(HEADERS, vid=session[0], accessToken=session[1])
    opener = urllib.request.build_opener(NoRedirect)
    info = require_json(request(opener, headers, "/book/info", {"bookId": args.book_id}))
    if str(info.get("bookId")) != args.book_id:
        raise ProbeError("Book identity mismatch.")
    toc = require_json(request(opener, headers, "/book/chapterInfos", body={
        "bookIds": [args.book_id], "synckeys": [0], "updateTimes": [0], "maxfreeIdx": [0]
    }))
    entries = [x for x in toc.get("data", []) if str(x.get("bookId")) == args.book_id]
    if len(entries) != 1:
        raise ProbeError("Missing or ambiguous chapter catalog.")
    entry = entries[0]
    chapters = entry.get("updated", [])
    if args.chapter_uid:
        chapter = next((x for x in chapters if str(x.get("chapterUid")) == args.chapter_uid), None)
    else:
        chapter = next((x for x in chapters if x.get("price") == 0 and x.get("wordCount", 0) >= 500), None)
    if not chapter:
        raise ProbeError("No matching chapter; specify a readable --chapter-uid.")
    if chapter.get("price") != 0 and chapter.get("paid") != 1 and info.get("paid") != 1:
        raise ProbeError("Probe supports only explicitly free or purchased chapters.")
    fmt = info.get("format")
    version = info.get("version")
    if fmt != "epub" or version is None:
        raise ProbeError("This probe currently covers EPUB with a server-provided version only.")
    params = {
        "bookId": args.book_id, "bookType": fmt, "bookVersion": version,
        "chapters": str(chapter["chapterUid"]), "stopAutoPayWhenBNE": 1,
    }
    meta, _, raw, envelope = request(opener, headers, "/book/chapterdownload", params)
    report = {
        "request": params,
        "catalog_synckey": entry.get("synckey"),
        "download": meta,
        "plaintext_verified": False,
    }
    if meta["http"] != 200 or meta.get("error_code", 0) != 0 or not zipfile.is_zipfile(io.BytesIO(raw)):
        print(json.dumps(report, ensure_ascii=False, indent=2))
        raise ProbeError("Download did not return a ZIP archive.")
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        report["entries"] = [{
            "name": x.filename, "bytes": x.file_size, "compressed_bytes": x.compress_size,
            "compression": x.compress_type, "encrypted": bool(x.flag_bits & 1),
        } for x in archive.infolist()]
    if args.verify_plaintext:
        report["plaintext_verification"] = verify_plaintext(
            raw, envelope, session[0], args.book_id, params["chapters"]
        )
        report["plaintext_verified"] = True
    if args.archive:
        fd = os.open(args.archive, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(raw)
    if args.auth_control:
        control, *_ = request(opener, HEADERS, "/book/chapterdownload", params)
        report["without_credentials"] = control
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ProbeError, OSError, sqlite3.Error, zipfile.BadZipFile):
        # Even library exceptions can contain file paths or response fragments.
        # ProbeError messages above are deliberately restricted to safe metadata.
        error = sys.exc_info()[1]
        print(str(error) if isinstance(error, ProbeError) else "Local I/O or archive validation failed.", file=sys.stderr)
        sys.exit(1)
