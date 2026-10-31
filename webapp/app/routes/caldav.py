"""CalDAV routes — HTTP endpoints for iOS Reminders sync.

Handles OPTIONS, PROPFIND, REPORT, GET, PUT, DELETE with Basic auth.
CSRF-exempt; does not create Flask sessions. Registered only when
``CALDAV_ENABLED`` is set.
"""

from __future__ import annotations

import hmac
import logging
import os
from collections.abc import Callable
from functools import wraps
from typing import Any
from urllib.parse import quote, unquote, urlsplit
from xml.sax.saxutils import escape

from defusedxml.ElementTree import ParseError
from defusedxml.ElementTree import fromstring as safe_xml_fromstring
from flask import Blueprint, Flask, Response, current_app, g, redirect, request, url_for

from app.exceptions import ConflictError, StorageError
from app.extensions import csrf
from app.services.caldav_service import (
    Resource,
    Snapshot,
    calendar_ctag,
    delete_item,
    get_resource,
    list_calendars,
    put_item,
)

logger = logging.getLogger(__name__)

caldav_bp = Blueprint("caldav", __name__, url_prefix="/caldav")
csrf.exempt(caldav_bp)

XML_CONTENT_TYPE = "application/xml; charset=utf-8"
ICS_CONTENT_TYPE = "text/calendar; charset=utf-8; component=VTODO"
DAV_HEADER = "1, 3, calendar-access"


# ----------------------------------------------------------------
# Auth
# ----------------------------------------------------------------
def caldav_credentials() -> tuple[str, str]:
    """(user, password) — CALDAV_* with APP_* as fallback."""
    user = os.environ.get("CALDAV_USER") or os.environ.get("APP_USER") or ""
    password = os.environ.get("CALDAV_PASSWORD") or os.environ.get("APP_PASSWORD") or ""
    return user, password


def _auth_user() -> str | None:
    """The authenticated username from Basic auth, or None."""
    auth = request.authorization
    user, password = caldav_credentials()
    if not auth or not user or not password:
        return None
    given_user = (auth.username or "").encode("utf-8")
    given_password = (auth.password or "").encode("utf-8")
    user_ok = hmac.compare_digest(given_user, user.encode("utf-8"))
    password_ok = hmac.compare_digest(given_password, password.encode("utf-8"))
    return user if user_ok and password_ok else None


def require_caldav_auth(f: Callable[..., Response]) -> Callable[..., Response]:
    """Check HTTP Basic auth and that the URL names the logged-in user."""

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
        if "username" in kwargs and kwargs["username"] != username:
            return Response("Not found", status=404)
        g.caldav_user = username
        if request.method == "OPTIONS":
            return _options()
        return f(*args, **kwargs)

    return decorated


@caldav_bp.after_request
def _log_request(resp: Response) -> Response:
    # gunicorn writes no access log; this is how to see what a device asks for.
    logger.info("CalDAV %s %s Depth=%s -> %s", request.method, request.path,
                request.headers.get("Depth", "-"), resp.status_code)
    return resp


@caldav_bp.errorhandler(ConflictError)
def _conflict(_e: ConflictError) -> Response:
    # The file changed under us (another client wrote); the device refetches.
    return Response("The todo file changed meanwhile", status=412)


@caldav_bp.errorhandler(StorageError)
def _storage_error(e: StorageError) -> Response:
    logger.warning("CalDAV storage error: %s", e)
    return Response("Storage unavailable", status=503)


def register_wellknown(app: Flask) -> None:
    @app.route("/.well-known/caldav", methods=["GET", "PROPFIND"])
    @app.route("/.well-known/caldav/", methods=["GET", "PROPFIND"])
    def caldav_wellknown() -> Any:
        if not current_app.config.get("CALDAV_ENABLED", False):
            return Response("Not found", status=404)
        return redirect(url_for("caldav.caldav_root"), code=301)


# ----------------------------------------------------------------
# URLs and XML helpers
# ----------------------------------------------------------------
def _principal_href(user: str) -> str:
    return f"/caldav/principal/{quote(user, safe='')}/"


def _home_href(user: str) -> str:
    return f"/caldav/calendars/{quote(user, safe='')}/"


def _calendar_href(user: str, list_name: str) -> str:
    return f"{_home_href(user)}{quote(list_name, safe='')}/"


def _resource_href(user: str, list_name: str, name: str) -> str:
    return f"{_calendar_href(user, list_name)}{quote(name, safe='')}.ics"


def _options() -> Response:
    resp = Response("", status=200)
    resp.headers["DAV"] = DAV_HEADER
    resp.headers["Allow"] = "OPTIONS, PROPFIND, REPORT, GET, PUT, DELETE"
    return resp


def _href(path: str) -> str:
    return f"<D:href>{escape(path)}</D:href>"


def _response(href: str, props: list[str]) -> str:
    return (
        f"<D:response>{_href(href)}<D:propstat><D:prop>{''.join(props)}</D:prop>"
        "<D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
    )


def _not_found(href: str) -> str:
    return f"<D:response>{_href(href)}<D:status>HTTP/1.1 404 Not Found</D:status></D:response>"


def _multistatus(responses: list[str]) -> Response:
    body = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" '
        'xmlns:CS="http://calendarserver.org/ns/" xmlns:APPLE="http://apple.com/ns/ical/">'
        f"{''.join(responses)}</D:multistatus>"
    )
    return Response(body, status=207, content_type=XML_CONTENT_TYPE)


def _depth() -> str:
    return request.headers.get("Depth", "0").strip()


# ----------------------------------------------------------------
# Discovery: root, principal, calendar home
# ----------------------------------------------------------------
@caldav_bp.route("/", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def caldav_root() -> Response:
    user = g.caldav_user
    return _multistatus([_response(request.path, [
        "<D:resourcetype><D:collection/></D:resourcetype>",
        f"<D:current-user-principal>{_href(_principal_href(user))}</D:current-user-principal>",
    ])])


@caldav_bp.route("/principal/<username>/", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def principal(username: str) -> Response:
    return _multistatus([_response(_principal_href(username), [
        f"<D:displayname>{escape(username)}</D:displayname>",
        "<D:resourcetype><D:collection/><D:principal/></D:resourcetype>",
        f"<D:current-user-principal>{_href(_principal_href(username))}</D:current-user-principal>",
        f"<C:calendar-home-set>{_href(_home_href(username))}</C:calendar-home-set>",
    ])])


def _calendar_props(user: str, list_name: str, snap: Snapshot) -> list[str]:
    return [
        f"<D:displayname>{escape(list_name)}</D:displayname>",
        "<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>",
        f"<CS:getctag>{calendar_ctag(list_name, snap)}</CS:getctag>",
        '<C:supported-calendar-component-set><C:comp name="VTODO"/>'
        "</C:supported-calendar-component-set>",
        "<D:supported-report-set>"
        "<D:supported-report><D:report><C:calendar-multiget/></D:report></D:supported-report>"
        "<D:supported-report><D:report><C:calendar-query/></D:report></D:supported-report>"
        "</D:supported-report-set>",
        f"<D:owner>{_href(_principal_href(user))}</D:owner>",
        "<D:current-user-privilege-set><D:privilege><D:all/></D:privilege>"
        "</D:current-user-privilege-set>",
    ]


@caldav_bp.route("/calendars/<username>/", methods=["OPTIONS", "PROPFIND"])
@require_caldav_auth
def calendar_home(username: str) -> Response:
    responses = [_response(_home_href(username), [
        "<D:resourcetype><D:collection/></D:resourcetype>",
        f"<D:current-user-principal>{_href(_principal_href(username))}</D:current-user-principal>",
    ])]
    if _depth() != "0":
        snap = Snapshot()
        for list_name in list_calendars(snap):
            responses.append(_response(
                _calendar_href(username, list_name),
                _calendar_props(username, list_name, snap),
            ))
    return _multistatus(responses)


# ----------------------------------------------------------------
# A list: PROPFIND and REPORT
# ----------------------------------------------------------------
def _resource_props(res: Resource, with_data: bool) -> list[str]:
    props = [
        f"<D:getetag>{escape(res.etag)}</D:getetag>",
        f"<D:getcontenttype>{ICS_CONTENT_TYPE}</D:getcontenttype>",
        f"<D:getcontentlength>{len(res.ics)}</D:getcontentlength>",
        "<D:resourcetype/>",
    ]
    if with_data:
        props.append(f"<C:calendar-data>{escape(res.ics.decode('utf-8'))}</C:calendar-data>")
    return props


def _known_list(list_name: str, snap: Snapshot) -> bool:
    return list_name in list_calendars(snap)


@caldav_bp.route("/calendars/<username>/<list_name>/", methods=["OPTIONS", "PROPFIND", "REPORT"])
@require_caldav_auth
def calendar_collection(username: str, list_name: str) -> Response:
    snap = Snapshot()
    if request.method == "REPORT":
        return _handle_report(username, list_name, snap)

    if not _known_list(list_name, snap):
        return Response("Not found", status=404)

    responses = [_response(
        _calendar_href(username, list_name), _calendar_props(username, list_name, snap),
    )]
    if _depth() != "0":
        for res in snap.resources(list_name):
            responses.append(_response(
                _resource_href(username, list_name, res.name), _resource_props(res, False),
            ))
    return _multistatus(responses)


def _parse_body() -> Any:
    try:
        return safe_xml_fromstring(request.get_data() or b"<empty/>")
    except ParseError:
        return None


def _wants_data(root: Any) -> bool:
    return any(el.tag == "{urn:ietf:params:xml:ns:caldav}calendar-data" for el in root.iter())


def _handle_report(username: str, list_name: str, snap: Snapshot) -> Response:
    root = _parse_body()
    if root is None:
        return Response("Malformed XML", status=400)

    with_data = _wants_data(root)
    if root.tag == "{urn:ietf:params:xml:ns:caldav}calendar-multiget":
        return _report_multiget(username, list_name, root, snap, with_data)
    if root.tag == "{urn:ietf:params:xml:ns:caldav}calendar-query":
        # The filter (iOS: VTODO, sometimes a time range) is not evaluated:
        # every visible task of the list is returned, the device filters.
        return _multistatus([
            _response(_resource_href(username, list_name, res.name), _resource_props(res, with_data))
            for res in snap.resources(list_name)
        ])
    return Response("Unsupported report", status=403)


def _report_multiget(username: str, list_name: str, root: Any, snap: Snapshot,
                     with_data: bool) -> Response:
    responses = []
    for el in root.iter("{DAV:}href"):
        href = (el.text or "").strip()
        name = unquote(urlsplit(href).path.rstrip("/").rsplit("/", 1)[-1])
        res = snap.get(list_name, name[:-4]) if name.endswith(".ics") else None
        if res is None:
            responses.append(_not_found(href))
        else:
            responses.append(_response(href, _resource_props(res, with_data)))
    return _multistatus(responses)


# ----------------------------------------------------------------
# A single task — GET / PUT / DELETE
# ----------------------------------------------------------------
@caldav_bp.route(
    "/calendars/<username>/<list_name>/<name>.ics",
    methods=["OPTIONS", "GET", "PUT", "DELETE"],
)
@require_caldav_auth
def calendar_resource(username: str, list_name: str, name: str) -> Response:
    if request.method == "GET":
        res = get_resource(list_name, name)
        if res is None:
            return Response("Not found", status=404)
        return Response(res.ics, status=200, content_type=ICS_CONTENT_TYPE,
                        headers={"ETag": res.etag})

    if request.method == "PUT":
        status, detail = put_item(
            list_name, name, request.get_data(),
            if_match=request.headers.get("If-Match"),
            if_none_match=request.headers.get("If-None-Match"),
        )
        if status in (200, 201):
            resp = Response("", status=status)
            if detail:
                resp.headers["ETag"] = detail
            return resp
        return Response(detail or "Error", status=status)

    status = delete_item(list_name, name, if_match=request.headers.get("If-Match"))
    return Response("", status=status)
