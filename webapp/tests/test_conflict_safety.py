"""Tests for concurrent-write safety.

These cover the failure that let a completed todo reappear as open: a stale
whole-file write landing on top of a newer state, unnoticed.
"""

import os
import sys
import tempfile
import time

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.exceptions import ConflictError
from app.routes.todo import _resolve_index
from app.services import storage
from app.services.parser import find_line_by_marker, marker_of
from app.services.undo_service import content_hash, pop_undo, push_undo, stamp_last_result


class TestMarkerResolution:
    """A line may carry several ``^id`` tokens — only the last one is ours."""

    def test_marker_is_the_trailing_id(self):
        line = '- [x] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ✅ 2025-12-24 ^gizxoy'
        assert marker_of(line) == 'gizxoy'

    def test_glued_id_is_only_a_fallback(self):
        line = '- [x] Meditation ✅ 2026-01-01^1d2hnl ^5szvah'
        assert marker_of(line) == '5szvah'

    def test_line_without_marker(self):
        assert marker_of('- [ ] Bettwäsche tauschen due:2026-01-10') is None

    def test_shared_block_link_does_not_capture_a_foreign_line(self):
        lines = [
            '- [ ] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ^aaa1',
            '- [ ] Laufen gehen [[Tagebuch/2025-12-22.md]] ^3pip9m ^bbb2',
        ]
        assert find_line_by_marker(lines, 'bbb2') == 1
        assert find_line_by_marker(lines, 'aaa1') == 0

    def test_falls_back_to_a_token_match(self):
        lines = ['- [ ] Nur ein Blocklink ^3pip9m ^aaa1']
        assert find_line_by_marker(lines, '3pip9m') == 0


class TestResolveIndex:
    """The index from a rendered page may be stale; the marker is not."""

    def test_marker_wins_over_a_shifted_index(self):
        lines = ['- [ ] Neu ^new1', '- [ ] Alt ^old1']
        # The page was rendered when "Alt" sat on line 0.
        assert _resolve_index(lines, 0, 'old1') == 1

    def test_index_is_used_when_no_marker_is_supplied(self):
        lines = ['- [ ] Neu ^new1', '- [ ] Alt ^old1']
        assert _resolve_index(lines, 0, None) == 0

    def test_unknown_marker_refuses_to_guess(self):
        """A marker that is gone means the todo is gone.

        Falling back to the index here is what put completed tasks back on the
        list: the index still resolved, just to whatever had moved into that
        slot.
        """
        lines = ['- [ ] Neu ^new1']
        assert _resolve_index(lines, 0, 'gone') is None

    def test_index_past_the_end_resolves_to_nothing(self):
        assert _resolve_index(['- [ ] Neu ^new1'], 7, None) is None


@pytest.fixture
def local_file(app):
    """Point storage at a temp file and return its path."""
    with tempfile.NamedTemporaryFile(mode='w', suffix='.md', delete=False) as f:
        f.write('- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n')
        path = f.name
    app.config['USE_WEBDAV'] = False
    app.config['TODO_PATH'] = path
    yield path
    os.unlink(path)


class TestWriteGuard:
    """A write is checked against the read it was derived from."""

    def test_foreign_write_is_reported_not_overwritten(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()

            # Another client stores something new.
            foreign = content.replace('- [ ] Zwei ^bbb2', '- [x] Zwei ✅ 2026-08-22 ^bbb2')
            os.utime(local_file, (0, 0))  # make sure the mtime differs
            with open(local_file, 'w', encoding='utf-8') as f:
                f.write(foreign)

            with pytest.raises(ConflictError):
                storage.write_content(content.replace('- [ ] Eins', '- [x] Eins'))

        with open(local_file, encoding='utf-8') as f:
            assert f.read() == foreign, 'the foreign write must survive'

    def test_undisturbed_write_succeeds(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            storage.write_content(content.replace('- [ ] Eins', '- [x] Eins'))

        with open(local_file, encoding='utf-8') as f:
            assert f.read().startswith('- [x] Eins')

    def test_force_write_bypasses_the_guard(self, app, local_file):
        with app.test_request_context():
            storage.read_content()
            os.utime(local_file, (0, 0))
            storage.force_write_content('- [ ] Überschrieben ^ccc3\n')

        with open(local_file, encoding='utf-8') as f:
            assert f.read() == '- [ ] Überschrieben ^ccc3\n'

    def test_the_first_read_of_a_request_is_what_the_write_is_checked_against(
            self, app, local_file):
        """A mutation reads the file more than once; the guard must span all of it.

        Route and service each call read_content(). If the later read reset the
        remembered validator, If-Match would only cover the microseconds
        between the innermost read and the write — and prove nothing about the
        state the request started from.
        """
        with app.test_request_context():
            storage.read_content()  # the route's read

            # A foreign writer lands between the two reads.
            with open(local_file, 'w', encoding='utf-8') as f:
                f.write('- [ ] Fremd ^ccc3\n')
            os.utime(local_file, (1, 1))

            content = storage.read_content()  # the service's read

            with pytest.raises(ConflictError):
                storage.write_content(content + '- [x] Eins ^aaa1\n')

    def test_a_second_write_in_one_request_is_not_a_false_conflict(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            storage.write_content(content + '- [ ] Drei ^ccc3\n')
            # The recurrence spawn writes again right after the toggle.
            storage.write_content(content + '- [ ] Drei ^ccc3\n- [ ] Vier ^ddd4\n')

        with open(local_file, encoding='utf-8') as f:
            assert '^ddd4' in f.read()


class TestStaleIndexToggle:
    """The reported bug: a tick on a page whose file has moved on.

    Every case here used to be a silent 200 that left the user's task open, or
    worse, flipped a different one.
    """

    def _client(self, app):
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True
        return client

    def test_marker_beats_a_shifted_index(self, app, local_file):
        """Another writer inserted a line above; the index now points at 'Eins'."""
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [ ] Neu von woanders ^ccc3\n- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n')

        # The page was rendered when "Eins" sat on line 0.
        response = self._client(app).post('/toggle/0', data={'marker': 'aaa1'})
        assert response.status_code in (200, 302)

        with open(local_file, encoding='utf-8') as f:
            lines = f.read().splitlines()
        assert lines[0] == '- [ ] Neu von woanders ^ccc3', 'the foreign line must not be touched'
        assert lines[1].startswith('- [x] Eins'), 'the clicked todo must be the one completed'

    def test_a_completed_foreign_todo_is_not_reopened(self, app, local_file):
        """The exact regression: the stale slot holds someone else's done task.

        Without the marker, `not is_done` was computed from that line and put a
        finished task back on the list.
        """
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [x] Fremd erledigt ✅ 2026-08-22 ^ccc3\n- [ ] Eins ^aaa1\n')

        self._client(app).post('/toggle/0', data={'marker': 'aaa1'})

        with open(local_file, encoding='utf-8') as f:
            content = f.read()
        assert '- [x] Fremd erledigt' in content, 'the foreign task must stay completed'
        assert '- [x] Eins' in content

    def test_a_vanished_todo_is_reported_not_guessed(self, app, local_file):
        """Deleted elsewhere: refuse, rather than tick whatever took its place."""
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [ ] Ganz woanders ^ccc3\n')

        response = self._client(app).post('/toggle/0', data={'marker': 'aaa1'})
        assert response.status_code == 409

        with open(local_file, encoding='utf-8') as f:
            assert f.read() == '- [ ] Ganz woanders ^ccc3\n', 'nothing may be written'

    def test_a_line_without_a_marker_still_toggles_by_index(self, app, local_file):
        """Hand-written lines carry no ^id; the index is all there is."""
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [ ] Von Hand geschrieben\n')

        self._client(app).post('/toggle/0', data={})

        with open(local_file, encoding='utf-8') as f:
            assert f.read().startswith('- [x] Von Hand geschrieben')

    def test_postpone_follows_the_marker_too(self, app, local_file):
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [ ] Neu von woanders ^ccc3\n- [ ] Eins ^aaa1\n')

        self._client(app).post('/postpone/0/today', data={'marker': 'aaa1'})

        with open(local_file, encoding='utf-8') as f:
            lines = f.read().splitlines()
        assert 'due:' not in lines[0], 'the foreign line must not be touched'
        assert 'due:' in lines[1]

    def test_batch_skips_what_is_gone_and_does_the_rest(self, app, local_file):
        """One vanished todo must not sink the whole selection."""
        with open(local_file, 'w', encoding='utf-8') as f:
            f.write('- [ ] Eins ^aaa1\n- [ ] Zwei ^bbb2\n')

        response = self._client(app).post('/api/toggle-batch', json={
            'line_indexes': [0, 1],
            'markers': ['aaa1', 'weg99'],
            'done': True,
        })
        assert response.status_code == 200
        payload = response.get_json()
        assert payload['updated'] == 1
        assert payload['failed'] == [1]

        with open(local_file, encoding='utf-8') as f:
            content = f.read()
        assert '- [x] Eins' in content
        assert '- [ ] Zwei' in content


class TestUndoGuard:
    """Undo restores a whole file — only over the state it was recorded for."""

    def test_stamp_records_the_produced_state(self, app):
        with app.test_request_context():
            push_undo('vorher', 'toggle')
            stamp_last_result('nachher')
            entry = pop_undo()
            assert entry['expected_hash'] == content_hash('nachher')

    def test_unstamped_entry_stays_unguarded(self, app):
        with app.test_request_context():
            push_undo('vorher', 'toggle')
            assert pop_undo()['expected_hash'] is None

    def test_write_stamps_the_entry(self, app, local_file):
        with app.test_request_context():
            content = storage.read_content()
            push_undo(content, 'toggle')
            produced = content.replace('- [ ] Eins', '- [x] Eins')
            storage.write_content(produced)
            assert pop_undo()['expected_hash'] == content_hash(produced)


class TestCsrfLifetime:
    """A page kept current by partial reloads must not silently lose its token."""

    def test_the_token_outlives_the_flask_wtf_default_hour(self):
        """The token is rendered once and never refreshed by the reloads.

        With the one-hour default, a page open longer than that looked fine but
        could no longer write: every POST came back 400 and a tapped checkbox
        reappeared unticked.
        """
        from app.config import Config

        assert Config.WTF_CSRF_TIME_LIMIT is None

    def test_a_rejected_token_is_named_so_the_client_can_retry(self, app):
        """CSRFProtect rejects before the view runs, so a retry writes nothing twice."""
        app.config['WTF_CSRF_ENABLED'] = True
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True

        response = client.post(
            '/toggle/0',
            data={'marker': 'aaa1'},
            headers={'X-CSRFToken': 'abgelaufen'},
        )

        assert response.status_code == 400
        assert response.get_json() == {'error': 'csrf_expired'}

    def test_a_really_expired_token_writes_nothing_and_says_so(self, app, local_file):
        """The reported failure, with the hour shortened to a second.

        This is the exact server-side sequence behind "The CSRF token has
        expired." in the production log: the write is refused, the file is
        untouched, and the answer names the cause so the client can recover.
        """
        app.config['WTF_CSRF_ENABLED'] = True
        # Zero, not one: itsdangerous stamps whole seconds and compares
        # `age > max_age`, so a one-second limit lets an age of exactly 1
        # through. With zero, any token that has crossed a second boundary is
        # expired, which a 1.1 s wait guarantees.
        app.config['WTF_CSRF_TIME_LIMIT'] = 0
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True

        token = client.get('/api/csrf-token').get_json()['csrf_token']
        time.sleep(1.1)

        response = client.post('/toggle/0', data={'marker': 'aaa1'},
                               headers={'X-CSRFToken': token})

        assert response.status_code == 400
        assert response.get_json() == {'error': 'csrf_expired'}
        with open(local_file, encoding='utf-8') as f:
            assert f.read().startswith('- [ ] Eins'), 'nothing may have been written'

        # What the client does next: fresh token, same call, and it lands.
        app.config['WTF_CSRF_TIME_LIMIT'] = None
        fresh = client.get('/api/csrf-token').get_json()['csrf_token']
        response = client.post('/toggle/0', data={'marker': 'aaa1'},
                               headers={'X-CSRFToken': fresh})

        assert response.status_code in (200, 302)
        with open(local_file, encoding='utf-8') as f:
            assert f.read().startswith('- [x] Eins')

    def test_a_form_post_still_gets_the_error_page(self, app):
        app.config['WTF_CSRF_ENABLED'] = True
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True

        response = client.post('/toggle/0', data={'csrf_token': 'abgelaufen'})

        assert response.status_code == 400
        assert b'CSRF' in response.data

    def test_a_fresh_token_can_be_fetched(self, app):
        client = app.test_client()
        with client.session_transaction() as sess:
            sess['logged_in'] = True

        response = client.get('/api/csrf-token')

        assert response.status_code == 200
        assert response.get_json()['csrf_token']

    def test_the_token_endpoint_needs_a_session(self, app):
        assert app.test_client().get('/api/csrf-token').status_code == 401


class TestConflictHandler:
    """A rejected write must read as a conflict, not as a server fault."""

    def test_api_conflict_is_409_json(self, app):
        @app.route('/api/_conflict_probe')
        def probe():
            raise ConflictError('alt', 'neu')

        response = app.test_client().get('/api/_conflict_probe')
        assert response.status_code == 409
        payload = response.get_json()
        assert payload['conflict'] is True
        assert 'Reload' in payload['error']

    def test_page_conflict_is_409_html(self, app):
        @app.route('/_conflict_probe')
        def probe():
            raise ConflictError('alt', 'neu')

        response = app.test_client().get('/_conflict_probe')
        assert response.status_code == 409
        assert b'changed elsewhere' in response.data
