"""Tests for the CalDAV HTTP routes."""

import os
import tempfile
import base64
from xml.etree.ElementTree import fromstring as et_fromstring

import pytest

from app import create_app


@pytest.fixture
def caldav_app(temp_todo_file):
    """Create app with caldav enabled for testing."""
    config_dir = tempfile.mkdtemp(prefix="caldav_test_")
    os.environ["CONFIG_PATH"] = os.path.join(config_dir, "settings.json")
    os.environ["TODOS_DB_PATH"] = temp_todo_file
    os.environ["CALDAV_USER"] = "testuser"
    os.environ["CALDAV_PASSWORD"] = "testpass"

    app = create_app("testing")
    app.config["TODO_PATH"] = temp_todo_file
    yield app

    os.environ.pop("TODOS_DB_PATH", None)
    os.environ.pop("CALDAV_USER", None)
    os.environ.pop("CALDAV_PASSWORD", None)
    os.environ.pop("CONFIG_PATH", None)
    import shutil
    shutil.rmtree(config_dir, ignore_errors=True)


@pytest.fixture
def client(caldav_app):
    return caldav_app.test_client()


def _auth():
    creds = base64.b64encode(b"testuser:testpass").decode("utf-8")
    return {"Authorization": f"Basic {creds}"}


def _propfind(client, path, body, **kwargs):
    return client.open(
        path, method="PROPFIND", data=body,
        content_type="application/xml",
        headers=_auth(), **kwargs,
    )


def _report(client, path, body, **kwargs):
    return client.open(
        path, method="REPORT", data=body,
        content_type="application/xml",
        headers=_auth(), **kwargs,
    )


class TestAuth:
    def test_returns_401_without_auth(self, client):
        resp = client.options("/caldav/")
        assert resp.status_code == 401

    def test_returns_401_with_wrong_auth(self, client):
        creds = base64.b64encode(b"wrong:wrong").decode()
        resp = client.options("/caldav/", headers={"Authorization": f"Basic {creds}"})
        assert resp.status_code == 401

    def test_returns_200_with_auth(self, client):
        resp = client.options("/caldav/", headers=_auth())
        assert resp.status_code == 200


class TestPropfind:
    def test_root(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<D:propfind xmlns:D="DAV:">'
            "<D:prop><D:displayname/><D:resourcetype/><D:current-user-principal/></D:prop>"
            "</D:propfind>"
        )
        resp = _propfind(client, "/caldav/", body)
        assert resp.status_code == 207

    def test_principal(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">'
            "<D:prop><D:displayname/><D:resourcetype/><C:calendar-home-set/></D:prop>"
            "</D:propfind>"
        )
        resp = _propfind(client, "/caldav/principal/testuser/", body)
        assert resp.status_code == 207

    def test_calendar_home(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">'
            "<D:prop><D:displayname/><D:resourcetype/><CS:getctag/></D:prop>"
            "</D:propfind>"
        )
        resp = _propfind(client, "/caldav/calendars/testuser/", body)
        assert resp.status_code == 207

    def test_collection_depth1(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<D:propfind xmlns:D="DAV:" xmlns:CS="http://calendarserver.org/ns/">'
            "<D:prop><D:displayname/><D:getetag/><D:resourcetype/><CS:getctag/></D:prop>"
            "</D:propfind>"
        )
        resp = _propfind(client, "/caldav/calendars/testuser/Reinschrift/", body)
        assert resp.status_code == 207


class TestGet:
    def test_get_item(self, client):
        resp = client.get("/caldav/calendars/testuser/Reinschrift/abc123.ics", headers=_auth())
        assert resp.status_code == 200
        assert b"Buy groceries" in resp.data

    def test_get_nonexistent(self, client):
        resp = client.get("/caldav/calendars/testuser/Reinschrift/nonexistent.ics", headers=_auth())
        assert resp.status_code == 404


class TestPut:
    def test_update_existing(self, client):
        ics = (
            "BEGIN:VCALENDAR\r\n"
            "VERSION:2.0\r\n"
            "PRODID:-//Test//EN\r\n"
            "BEGIN:VTODO\r\n"
            "UID:abc123\r\n"
            "SUMMARY:Updated groceries\r\n"
            "STATUS:NEEDS-ACTION\r\n"
            "END:VTODO\r\n"
            "END:VCALENDAR\r\n"
        )
        resp = client.put(
            "/caldav/calendars/testuser/Reinschrift/abc123.ics",
            data=ics, content_type="text/calendar", headers=_auth(),
        )
        assert resp.status_code in (200, 201)


class TestDelete:
    def test_delete_nonexistent(self, client):
        resp = client.delete(
            "/caldav/calendars/testuser/Reinschrift/nonexistent.ics", headers=_auth(),
        )
        assert resp.status_code == 404

    def test_delete_existing(self, client):
        resp = client.delete("/caldav/calendars/testuser/Reinschrift/abc123.ics", headers=_auth())
        assert resp.status_code == 204


class TestSafety:
    def test_other_username_in_url(self, client):
        resp = _propfind(client, "/caldav/calendars/someoneelse/", "")
        assert resp.status_code == 404

    def test_list_names_are_escaped(self, client, temp_todo_file):
        with open(temp_todo_file, "w") as f:
            f.write("- [ ] Task & <more> +R&D<x> ^m1\n")
        resp = client.open("/caldav/calendars/testuser/Reinschrift/", method="REPORT",
                           data='<C:calendar-query xmlns:D="DAV:" '
                                'xmlns:C="urn:ietf:params:xml:ns:caldav">'
                                "<D:prop><C:calendar-data/></D:prop></C:calendar-query>",
                           headers=_auth())
        assert resp.status_code == 207
        root = et_fromstring(resp.data)  # well-formed XML
        [data] = [el.text for el in root.iter("{urn:ietf:params:xml:ns:caldav}calendar-data")]
        assert "SUMMARY:Task & <more> +R&D<x>" in data

    def test_only_one_list(self, client):
        resp = client.open("/caldav/calendars/testuser/", method="PROPFIND",
                           headers={**_auth(), "Depth": "1"})
        root = et_fromstring(resp.data)
        names = [el.text for el in root.iter("{DAV:}displayname")]
        assert names == ["Reinschrift"]

    def test_content_length_is_real(self, client):
        resp = client.open("/caldav/calendars/testuser/Reinschrift/", method="PROPFIND",
                           headers={**_auth(), "Depth": "1"})
        root = et_fromstring(resp.data)
        lengths = [int(el.text) for el in root.iter("{DAV:}getcontentlength")]
        assert lengths and all(n > 0 for n in lengths)

    def test_settings_page_does_not_show_the_password(self, caldav_app):
        with caldav_app.test_request_context("/"):
            ctx = {}
            for fn in caldav_app.template_context_processors[None]:
                ctx.update(fn())
        assert "testpass" not in ctx["caldav_help_text"]
        assert "testuser" in ctx["caldav_help_text"]


class TestReport:
    def test_multiget(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">'
            "<D:prop><D:getetag/><C:calendar-data/></D:prop>"
            "<D:href>/caldav/calendars/testuser/Reinschrift/abc123.ics</D:href>"
            "</C:calendar-multiget>"
        )
        resp = _report(client, "/caldav/calendars/testuser/Reinschrift/", body)
        assert resp.status_code == 207
        assert b"Buy groceries" in resp.data

    def test_multiget_unknown_href_is_404(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">'
            "<D:prop><D:getetag/></D:prop>"
            "<D:href>/caldav/calendars/testuser/Reinschrift/gone.ics</D:href>"
            "</C:calendar-multiget>"
        )
        resp = _report(client, "/caldav/calendars/testuser/Reinschrift/", body)
        assert resp.status_code == 207
        assert b"404 Not Found" in resp.data

    def test_calendar_query(self, client):
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">'
            "<D:prop><D:getetag/><C:calendar-data/></D:prop>"
            "<C:filter>"
            '<C:comp-filter name="VCALENDAR">'
            '<C:comp-filter name="VTODO"/>'
            "</C:comp-filter>"
            "</C:filter>"
            "</C:calendar-query>"
        )
        resp = _report(client, "/caldav/calendars/testuser/Reinschrift/", body)
        assert resp.status_code == 207
        assert b"Buy groceries" in resp.data


class TestWellKnown:
    def test_redirects(self, client):
        resp = client.get("/.well-known/caldav", headers=_auth())
        assert resp.status_code in (301, 302, 308)

    def test_disabled(self, caldav_app, temp_todo_file):
        caldav_app.config["CALDAV_ENABLED"] = False
        with caldav_app.test_client() as c:
            resp = c.get("/.well-known/caldav")
            assert resp.status_code == 404