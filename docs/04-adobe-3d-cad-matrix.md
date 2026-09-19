# 04 — Adobe / 3D / CAD Connector Matrix (Verified)

## Adobe (developer.adobe.com)
- Photoshop: UXP plugin + UXP Scripting (.psjs, ES6, `batchPlay` for unexposed DOM, `executeAsModal` for mutations) + legacy ExtendScript/VB/AppleScript + REST API cloud batch. UXP async, non-blocking.
- InDesign (+ Server headless): UXP scripts + plugins, HTML/CSS/JS, Marketplace distributable.
- Premiere Pro: UXP API (sequences/tracks) + CEP panels + C++ SDK. UXP scripting from 25.1.
- Pattern: UXP panel = live hand, script/REST = headless hand. Same as Office.js/COM split.

## 3D / Engines
- Blender (docs.blender.org): `blender -b --python s.py` + `import bpy` + headless module + Operators/Panels live. Best headless citizen.
- Unreal (docs.unrealengine.com): Python Editor Script Plugin (`import unreal`, editor-only) + CLI `-ExecutePythonScript=` (full editor) / `-run=pythonscript -script=` (headless commandlet) / `-ExecCmds="py ..."` (stay open live) + Remote Control HTTP `PUT /remote/object/call` + WS `ws://127.0.0.1:30020`. Ideal connector.
- Maya (help.autodesk.com): `import maya.cmds` + `maya.mel.eval()`, Script Editor/Shelf/userSetup.py, batch mode.
- 3ds Max: MAXScript + `pymxs` wrapper. CLI `-silent -mxs`, `-U MAXScript`, `-mi` minimized farm render.

## CAD
- AutoCAD: AutoLISP + VBA/ActiveX (`ThisDrawing`), .NET ObjectARX, `accoreconsole.exe` headless (Win, ships with AutoCAD), `pyautocad` COM, APS REST. Python->LISP->accoreconsole = real DWG headless; Civil 3D objects need full-session .NET.

## SDK openness (from thread Q)
- Yes: Adobe UXP/CEP docs + samples open; Blender GPL + docs open; Unreal docs + `unreal` module + Remote Control HTTP/WS documented with sample HTML/JS; Autodesk Learn + MAXScript/pymxs + ObjectARX docs open; PowerShell MIT. Apps proprietary, automation surfaces documented.

## Start order
Office + browser + Photoshop + Blender + Unreal Remote Control. Add CAD (accoreconsole) when DWG needed.
