import errno
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import AsyncMock, patch

import rlm


class FileHelperTests(unittest.IsolatedAsyncioTestCase):
    async def test_direct_io_uses_user_files_without_host_requests(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "file.txt"
            with patch.object(rlm, "host_request", new_callable=AsyncMock) as host:
                await rlm.write(path, "first")
                self.assertEqual(await rlm.read(path), "first")
                await rlm.write_once(path, "second")
                self.assertEqual(await rlm.read_once(path), "second")
                host.assert_not_awaited()

    async def test_read_failures_use_host_with_once_semantics(self):
        for reader, extra in ((rlm.read, {}), (rlm.read_once, {"once": True})):
            with self.subTest(reader=reader.__name__):
                with patch("builtins.open", side_effect=OSError(errno.EACCES, "denied")), patch.object(
                    rlm, "host_request", new_callable=AsyncMock, return_value={"content": "host bytes"}
                ) as host:
                    self.assertEqual(await reader("file.txt"), "host bytes")
                    host.assert_awaited_once_with(
                        "fs.read", {"path": "file.txt", "errno": errno.EACCES, **extra}
                    )

    async def test_write_failures_use_host_with_once_semantics(self):
        for writer, extra in ((rlm.write, {}), (rlm.write_once, {"once": True})):
            with self.subTest(writer=writer.__name__):
                with patch("builtins.open", side_effect=OSError(errno.EACCES, "denied")), patch.object(
                    rlm, "host_request", new_callable=AsyncMock, return_value={}
                ) as host:
                    self.assertIsNone(await writer("file.txt", "content"))
                    host.assert_awaited_once_with(
                        "fs.write", {"filePath": "file.txt", "content": "content", **extra}
                    )

    async def test_host_denial_surfaces_as_permission_error(self):
        for helper, args in (
            (rlm.read, ("file.txt",)),
            (rlm.read_once, ("file.txt",)),
            (rlm.write, ("file.txt", "content")),
            (rlm.write_once, ("file.txt", "content")),
        ):
            with self.subTest(helper=helper.__name__):
                with patch("builtins.open", side_effect=OSError(errno.EACCES, "denied")), patch.object(
                    rlm, "host_request", new_callable=AsyncMock, side_effect=RuntimeError("host denied")
                ):
                    with self.assertRaisesRegex(PermissionError, "host denied"):
                        await helper(*args)
