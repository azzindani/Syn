// `save` and `close`: the two things this helper did not do at all, and
// now does inside tight limits.
//
// The rule used to be "never saves, never closes", and it came from two
// incidents. Office COM servers are one per user, so the Excel this helper
// drives is the Excel the person has open, and closing or quitting there can
// take their own work with it. And `export` once used SaveAs, which MOVES the
// open workbook to the new path: the person's file silently became the
// export. So:
//
//   save   writes a document to its OWN file, in its OWN format -- Save,
//          never SaveAs. Refused for a document that was never saved (there
//          is no file of its own, and Save would put up a dialog), for one
//          opened read-only, and for a CSV, which keeps one sheet's values
//          and would drop everything else in the workbook.
//   close  closes a document THIS helper opened, and only once it is saved.
//          One the person already had open is theirs to close; unsaved
//          changes are refused rather than thrown away. The application is
//          never quit, whatever is left open in it.
//
// Neither stops for a human: the person asked for both to go ahead.
//
// STATUS: compiles (net8.0-windows); run against live Excel, Word and
// PowerPoint on Windows (scripts/live-office-peak.ps1 has the steps).
using System;
using System.Collections.Generic;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        /// <summary>Documents this helper opened itself, by name: `open`
        /// adds to it when it really opened one, not when it found it open
        /// already. A helper started later does not know what an earlier one
        /// opened, and so refuses to close it -- the safe way to be wrong.</summary>
        private static readonly HashSet<string> OpenedHere = new(StringComparer.OrdinalIgnoreCase);

        private static void RefuseSave(string name, string path, bool readOnly)
        {
            if (string.IsNullOrEmpty(path))
                throw new InvalidOperationException(
                    $"not saved: {name} has never been saved to a file, so it has no file of its own to save to; export it with a format and a path instead");
            if (readOnly)
                throw new InvalidOperationException(
                    $"not saved: {name} is open read-only (another program may have it); export a copy instead");
        }

        private static string SaveExcel(dynamic wb)
        {
            string name = wb.Name, path = wb.Path;
            RefuseSave(name, path, (bool)wb.ReadOnly);
            if (name.EndsWith(".csv", StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException(
                    $"not saved: {name} is a CSV, which keeps only the first sheet's values; export it as xlsx to keep everything");
            wb.Save();
            return Ok($"saved {name} to {(string)wb.FullName}");
        }

        private static string SaveWord(dynamic doc)
        {
            string name = doc.Name, path = doc.Path;
            RefuseSave(name, path, (bool)doc.ReadOnly);
            doc.Save();
            return Ok($"saved {name} to {(string)doc.FullName}");
        }

        private static string SavePres(dynamic pres)
        {
            string name = pres.Name, path = pres.Path;
            // MsoTriState: -1 is true.
            RefuseSave(name, path, (int)pres.ReadOnly == -1);
            pres.Save();
            return Ok($"saved {name} to {(string)pres.FullName}");
        }

        private static string CloseDoc(string name, bool saved, string appName, Action close)
        {
            if (!OpenedHere.Contains(name))
                throw new InvalidOperationException(
                    $"not closed: {name} was not opened by Syn -- it was open already, and only the user closes it");
            if (!saved)
                throw new InvalidOperationException(
                    $"not closed: {name} has unsaved changes; save it first with struct verb save, or leave it open");
            var key = _app + ":" + name;
            close();
            OpenedHere.Remove(name);
            ForgetUndo(key);
            return Ok($"closed {name}; {appName} stays open");
        }

        // SaveChanges false in each: the document is already saved (checked
        // above), so this can only stop a prompt, never lose work.
        private static string CloseExcel(dynamic wb) =>
            CloseDoc((string)wb.Name, (bool)wb.Saved, "Excel", () => wb.Close(false));

        private static string CloseWord(dynamic doc) =>
            CloseDoc((string)doc.Name, (bool)doc.Saved, "Word", () => doc.Close(0)); // wdDoNotSaveChanges

        // MsoTriState again: Saved is -1 when there is nothing unsaved.
        private static string ClosePres(dynamic pres) =>
            CloseDoc((string)pres.Name, (int)pres.Saved == -1, "PowerPoint", () => pres.Close());
    }
}
