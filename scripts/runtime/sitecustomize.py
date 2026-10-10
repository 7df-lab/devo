"""Activate wheel .pth files in the private dependency directory.

PYTHONPATH alone does not activate .pth files. In particular, pywin32 needs
its bootstrap to expose pywintypes and register its private DLL directory.
"""

import os
import site

site.addsitedir(os.path.dirname(__file__))
