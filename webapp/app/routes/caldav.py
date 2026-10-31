"""CalDAV routes — HTTP endpoints for iOS Reminders sync.

Handles PROPFIND, REPORT, GET, PUT, DELETE with Basic auth.
CSRF‑exempt; does not create Flask sessions.
"""

from __future__ import annotations

import hmac
import logging
import os
from datetime import datetime
from functools import wraps
from typing import Any

from defusedxml.ElementTree import fromstring as safe_xml_fromstring
from flask import (
    Blueprint,
    Response,
    abort,
    current_app,
    g,
    make_response,
    request,
)

from app.services.caldav_service import (
    INBOX,
    CalendarInfo,
    calendar_ctag,
    calendar_items,
    delete_item,
    get_item_etag,
    get_item_ics,
    list_calendars,
    principal_url,
    calendar_home_url,
    put_item,
    ensure_markers,
)
from app.services.ical_service import vtodo_uid

logger = logging.getLogger(__name__)

caldav_bp = Blueprint("caldav", __name__, url_prefix="/caldav")

# ----------------------------------------------------------------
# XML namespaces
# ----------------------------------------------------------------
NS = {
    "D": "DAV:",
    "C": "urn:ietf:params:xml:ns:caldav",
    "CS": "http://calendarserver.org/ns/",
    "APPLE": "http://apple.com/ns/ical/",
}

XML_CONTENT_TYPE = "application/xml; charset=utf-8"
TEXT_XML = "text/xml"


# ----------------------------------------------------------------
# Auth
# ----------------------------------------------------------------
def _auth_user() -> str | None:
    """Extract the authenticated username from Basic auth, or None."""
    auth = request.authorization
    if not auth:
        return None

    username = (os.environ.get("CALDAV_USER") or os.environ.get("APP_USER") or "")
    password = (os.environ.get("CALDAV_PASSWORD") or os.environ.get("APP_PASSWORD") or "")

    if not username or not password:
        return None

    if not hmac.compare_digest(auth.username, username):
        return None
    if not hmac.compare_digest(auth.password, password):
        return None

    return username


def require_caldav_auth(f):
    """Decorator that checks HTTP Basic auth for CalDAV endpoints."""

    @wraps(f)
    def decorated(*args: Any, **kwargs: Any) -> Response:
        if not current_app.config.get("CALDAV_ENABLED", False):
            return Response("CalDAV disabled", status=404)

        username = _auth_user()
        if not username:
            return Response(
                "Unauthorized",
                status=401,
                headers={"WWW-Authenticate": 'Basic realm="Reinschrift CalDAV"'},
            )
        g.caldav_user = username
        g.caldav_base = request.url_root
        return f(*args, **kwargs)

    return decorated


# ----------------------------------------------------------------
# Well‑known redirect
# ----------------------------------------------------------------
from app.extensions import csrf  # noqa: E402
from flask import redirect, url_for  # noqa: E402

# Exempt entire blueprint from CSRF
csrf.exempt(caldav_bp)


def _register_wellknown(app):
    @app.route("/.well-known/caldav")
    @app.route("/.well-known/caldav/")
    def caldav_wellknown():
        if not current_app.config.get("CALDAV_ENABLED", False):
            abort(404)
        return redirect(url_for("caldav.caldav_root", _external=True))


# ----------------------------------------------------------------
# XML helpers
# ----------------------------------------------------------------
def _xml_doc(body: str) -> str:
    return f'<?xml version="1.0" encoding="UTF-8"?>\n{body}'


def _multistatus(body: str) -> str:
    return _xml_doc(
        f'<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" '
        f'xmlns:CS="http://calendarserver.org/ns/" '
        f'xmlns:APPLE="http://apple.com/ns/ical/">\n{body}\n</D:multistatus>'
    )


def _href(path: str) -> str:
    return f"<D:href>{path}</D:href>"


def _propstat(props_xml: str, status: str = "HTTP/1.1 200 OK") -> str:
    return (
        f"<D:propstat>\n"
        f"<D:prop>{props_xml}</D:prop>\n"
        f"<D:status>{status}</D:status>\n"
        f"</D:propstat>"
    )


def _response(href: str, propstat_xml: str) -> str:
    return (
        f"<D:response>\n"
        f"{_href(href)}\n"
        f"{propstat_xml}\n"
        f"</D:response>"
    )


_PARSE_XML_CACHE_PREFIX = "__caldav_xml_cache"


def _parse_propfind_body() -> tuple[list[str], str | None]:
    """Extract requested properties and filter from a PROPFIND body.

    Returns (prop_names, filter_element_raw).
    """
    body = request.get_data(as_text=True) or ""
    if not body.strip():
        return ["*"], None

    root = safe_xml_fromstring(body)
    props: list[str] = []

    for propfind in root.iter():
        tag = propfind.tag
        if tag.endswith("}prop"):
            for child in propfind:
                local = child.tag.split("}", 1)[-1] if "}" in child.tag else child.tag
                # Also include namespace-qualified names for common properties
                ns_prefix = child.tag.split("}")[0].lstrip("{") if "}" in child.tag else ""
                if ns_prefix == "DAV:":
                    props.append(local)
                elif ns_prefix == "http://calendarserver.org/ns/":
                    props.append(local)
                elif ns_prefix == "urn:ietf:params:xml:ns:caldav":
                    props.append(local)
                elif ns_prefix == "http://apple.com/ns/ical/":
                    props.append(local)
                else:
                    props.append(local)

    filter_raw = None
    for filt in root.iter():
        if filt.tag.split("}")[-1] == "filter":
            from xml.etree.ElementTree import tostring
            filter_raw = tostring(filt, encoding="unicode")

    if not props:
        return ["*"], filter_raw

    return props, filter_raw


# ----------------------------------------------------------------
# CalDAV root — principal discovery
# ----------------------------------------------------------------
@caldav_bp.route("/", methods=["OPTIONS", "PROPFIND"])
@caldav_bp.route("", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def caldav_root():
    username = g.caldav_user
    base = g.caldav_base

    if request.method == "OPTIONS":
        resp = make_response("")
        resp.headers["DAV"] = "1, 3, calendar-access"
        resp.headers["Allow"] = "OPTIONS, PROPFIND"
        return resp

    principal = principal_url(username, base)
    props: list[str] = []

    props.append(f"<D:displayname>{username}</D:displayname>")
    props.append(
        "<D:resourcetype>"
        "<D:collection/><C:calendar-access/>"
        "</D:resourcetype>"
    )
    props.append(f"<D:current-user-principal>{_href(principal)}</D:current-user-principal>")

    return Response(
        _multistatus(
            _response(request.path, _propstat("\n".join(props)))
        ),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


# ----------------------------------------------------------------
# Principal
# ----------------------------------------------------------------
@caldav_bp.route("/principal/<username>/", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def principal(username):
    if request.method == "OPTIONS":
        resp = make_response("")
        resp.headers["DAV"] = "1, 3, calendar-access"
        resp.headers["Allow"] = "OPTIONS, PROPFIND"
        return resp

    home = calendar_home_url(username, g.caldav_base)
    props: list[str] = []
    props.append(f"<D:displayname>{username}</D:displayname>")
    props.append(
        "<D:resourcetype>"
        "<D:principal/>"
        "</D:resourcetype>"
    )
    props.append(f"<C:calendar-home-set>{_href(home)}</C:calendar-home-set>")
    props.append("<C:calendar-user-address-set>"
                 f"{_href(f'mailto:{username}')}"
                 "</C:calendar-user-address-set>")

    return Response(
        _multistatus(
            _response(request.path, _propstat("\n".join(props)))
        ),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


# ----------------------------------------------------------------
# Calendar home — list of calendars
# ----------------------------------------------------------------
@caldav_bp.route("/calendars/<username>/", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def calendar_home(username):
    if request.method == "OPTIONS":
        resp = make_response("")
        resp.headers["DAV"] = "1, 3, calendar-access"
        resp.headers["Allow"] = "OPTIONS, PROPFIND"
        return resp

    ensure_markers()

    base = g.caldav_base
    calendars = list_calendars()
    responses: list[str] = []

    for cal in calendars:
        href = f"/caldav/calendars/{username}/{cal.name}/"
        ctag = calendar_ctag(cal.name)
        props: list[str] = []
        props.append(f"<D:displayname>{cal.display_name}</D:displayname>")
        props.append(
            "<D:resourcetype>"
            "<D:collection/>"
            "<C:calendar/>"
            "</D:resourcetype>"
        )
        props.append(f"<CS:getctag>{ctag}</CS:getctag>")
        props.append(f"<APPLE:calendar-color>{cal.color}</APPLE:calendar-color>")
        props.append(
            "<C:supported-calendar-component-set>"
            '<C:comp name="VTODO"/>'
            "</C:supported-calendar-component-set>"
        )
        props.append(
            "<D:supported-report-set>"
            "<D:supported-report>"
            "<D:report><C:calendar-multiget/></D:report>"
            "</D:supported-report>"
            "<D:supported-report>"
            "<D:report><C:calendar-query/></D:report>"
            "</D:supported-report>"
            "</D:supported-report-set>"
        )
        props.append(
            "<C:schedule-calendar-transp>"
            "<C:opaque/>"
            "</C:schedule-calendar-transp>"
        )
        props.append("<D:owner>" + _href(principal_url(username, base)) + "</D:owner>")
        props.append(f"<D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set>")

        responses.append(
            _response(href, _propstat("\n".join(props)))
        )

    return Response(
        _multistatus("\n".join(responses)),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


# ----------------------------------------------------------------
# Single calendar — items + reports
# ----------------------------------------------------------------
def _calendar_item_props(item_title: str, ics_bytes: bytes, etag: str, username: str,
                         list_name: str, marker: str) -> list[str]:
    props: list[str] = []
    props.append(f"<D:getetag>{etag}</D:getetag>")
    props.append("<D:getcontenttype>text/calendar; charset=utf-8; component=VTODO</D:getcontenttype>")
    ics_text = ics_bytes.decode("utf-8")
    props.append(f"<C:calendar-data>{_xml_escape(ics_text)}</C:calendar-data>")

    from xml.etree.ElementTree import fromstring as et_fromstring
    try:
        doc = et_fromstring(ics_bytes)
        ics_str = ics_bytes.decode("utf-8")
    except Exception:
        ics_str = ""

    content_len = len(ics_str.encode("utf-8"))
    props.append(f"<D:getcontentlength>{content_len}</D:getcontentlength>")
    props.append("<D:resourcetype/>")

    return props


def _xml_escape(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")


@caldav_bp.route("/calendars/<username>/<list_name>/", methods=["OPTIONS", "PROPFIND", "REPORT"])
@require_caldav_auth
def calendar_collection(username, list_name):
    if request.method == "OPTIONS":
        resp = make_response("")
        resp.headers["DAV"] = "1, 3, calendar-access"
        resp.headers["Allow"] = "OPTIONS, PROPFIND, GET, PUT, DELETE, REPORT"
        return resp

    ensure_markers()

    if request.method == "REPORT":
        return _handle_report(username, list_name)

    depth = request.headers.get("Depth", "0")

    base_path = request.path.rstrip("/") + "/"

    responses: list[str] = []

    # Depth 0 — the collection itself
    ctag = calendar_ctag(list_name)
    props: list[str] = []
    props.append(f"<D:displayname>{list_name}</D:displayname>")
    props.append(
        "<D:resourcetype>"
        "<D:collection/>"
        "<C:calendar/>"
        "</D:resourcetype>"
    )
    props.append(f"<CS:getctag>{ctag}</CS:getctag>")
    props.append(
        "<C:supported-calendar-component-set>"
        '<C:comp name="VTODO"/>'
        "</C:supported-calendar-component-set>"
    )
    responses.append(
        _response(base_path, _propstat("\n".join(props)))
    )

    if depth == "1":
        items = calendar_items(list_name, include_done=True)
        for item, ics, etag in items:
            if not item.marker:
                continue
            href = f"{base_path}{item.marker}.ics"
            ip = _calendar_item_props(item.title, ics, etag, username, list_name, item.marker)
            responses.append(
                _response(href, _propstat("\n".join(ip)))
            )

    return Response(
        _multistatus("\n".join(responses)),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


def _handle_report(username, list_name):
    body = request.get_data(as_text=True) or ""
    root = safe_xml_fromstring(body)

    # Determine report type
    tag_local = root.tag.split("}")[-1] if "}" in root.tag else root.tag

    if tag_local == "calendar-multiget":
        return _report_multiget(username, list_name, root)
    elif tag_local == "calendar-query":
        return _report_query(username, list_name, root)
    else:
        return Response("Unsupported report", status=400)


def _report_multiget(username, list_name, root) -> Response:
    """Handle calendar-multiget REPORT — fetch specific items."""
    base_path = request.path.rstrip("/") + "/"
    responses: list[str] = []

    for href_elem in root.iter():
        if not href_elem.tag.endswith("}href"):
            continue
        href_text = (href_elem.text or "").strip()
        if not href_text or ".ics" not in href_text:
            continue

        raw = href_text.rstrip("/").rsplit("/", 1)[-1]
        if raw.endswith(".ics"):
            raw = raw[:-4]

        ics = get_item_ics(list_name, raw)
        if ics is None:
            continue

        etag = get_item_etag(list_name, raw) or ""
        from app.services.ical_service import parse_vtodo_from_ics
        from app.services.parser import find_line_by_marker
        from app.services.storage import read_content

        content = read_content()
        lines = content.splitlines() if content else []
        idx = find_line_by_marker(lines, raw)
        title = "(unknown)"
        if idx is not None:
            from app.services.parser import parse_line as pl
            item = pl(lines[idx], idx)
            if item:
                title = item.title

        ip = _calendar_item_props(title, ics, etag, username, list_name, raw)
        responses.append(
            _response(href_text, _propstat("\n".join(ip)))
        )

    if not responses:
        return Response(
            _multistatus(
                _response(base_path, _propstat("", "HTTP/1.1 404 Not Found"))
            ),
            content_type=XML_CONTENT_TYPE,
            status=207,
        )

    return Response(
        _multistatus("\n".join(responses)),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


def _report_query(username, list_name, root) -> Response:
    """Handle calendar-query REPORT — fetch items matching a filter.

    iOS uses this with a time-range filter to get recent items.
    For simplicity, return all items in the list — the iOS client
    will filter client-side.
    """
    base_path = request.path.rstrip("/") + "/"
    items = calendar_items(list_name, include_done=True)
    responses: list[str] = []

    for item, ics, etag in items:
        if not item.marker:
            continue
        href = f"{base_path}{item.marker}.ics"
        ip = _calendar_item_props(item.title, ics, etag, username, list_name, item.marker)
        responses.append(
            _response(href, _propstat("\n".join(ip)))
        )

    return Response(
        _multistatus("\n".join(responses)),
        content_type=XML_CONTENT_TYPE,
        status=207,
    )


# ----------------------------------------------------------------
# Single VTODO resource — GET / PUT / DELETE
# ----------------------------------------------------------------
@caldav_bp.route(
    "/calendars/<username>/<list_name>/<uid>.ics",
    methods=["OPTIONS", "GET", "PUT", "DELETE"],
)
@require_caldav_auth
def calendar_resource(username, list_name, uid):
    if request.method == "OPTIONS":
        resp = make_response("")
        resp.headers["DAV"] = "1, 3, calendar-access"
        resp.headers["Allow"] = "OPTIONS, GET, PUT, DELETE"
        return resp

    if request.method == "GET":
        ics = get_item_ics(list_name, uid)
        if ics is None:
            abort(404)

        etag = get_item_etag(list_name, uid)
        resp = make_response(ics)
        resp.headers["Content-Type"] = "text/calendar; charset=utf-8; component=VTODO"
        if etag:
            resp.headers["ETag"] = etag
        return resp

    if request.method == "PUT":
        ics_bytes = request.get_data()
        if_match = request.headers.get("If-Match")
        if_none_match = request.headers.get("If-None-Match")

        if if_none_match == "*":
            existing = get_item_ics(list_name, uid)
            if existing is not None:
                return Response("Resource exists", status=412)

        if if_match:
            current_etag = get_item_etag(list_name, uid)
            if current_etag and current_etag != if_match:
                return Response("ETag mismatch", status=412)

        status, etag_or_error = put_item(list_name, uid, ics_bytes, if_match)
        if status in (200, 201):
            resp = make_response("", status)
            if etag_or_error:
                resp.headers["ETag"] = etag_or_error
            return resp
        return Response(etag_or_error or "Error", status=status)

    if request.method == "DELETE":
        if_match = request.headers.get("If-Match")
        if if_match:
            current_etag = get_item_etag(list_name, uid)
            if current_etag and current_etag != if_match:
                return Response("ETag mismatch", status=412)

        deleted = delete_item(list_name, uid)
        if deleted:
            return make_response("", 204)
        abort(404)

    abort(405)