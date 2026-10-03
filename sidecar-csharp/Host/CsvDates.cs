// Dates in a CSV that Excel would read in the wrong order.
//
// `12/1/2023` is the 1st of December in a file written month first and the
// 12th of January on a machine set day first, and Excel picks by the machine,
// quietly. A monthly series whose days are all 1 became 2023-01-12, 2023-02-12
// and so on: every date wrong, nothing to say so. A 50-message run spent 73
// steps on a scratch sheet working out what had happened to a column that was
// supposed to be text, and its next turn was stopped by the repeated-call gate
// while it was still trying to write the formula that undoes it.
//
// So the import is told, column by column, what to do with a column made
// entirely of d/m/yyyy-shaped values: if one part is over 12 somewhere in the
// file, that part is the day and the order is known; if neither is, the file
// does not say, and the column is imported as text with a note so that no
// order is guessed. Columns of anything else, ISO dates included, are left to
// Excel as before.

using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private static readonly Regex SlashDate = new(@"^\d{1,2}/\d{1,2}/\d{4}(?:[ T].*)?$", RegexOptions.Compiled);

        /// <summary>Per-column import types for the QueryTable (1 general,
        /// 2 text, 3 month-day-year, 4 day-month-year), and a note for the
        /// reply; null when every column can be left to Excel.</summary>
        internal static (object[]? types, string note) SniffDateColumns(string path, char delim, bool utf8)
        {
            using var reader = new StreamReader(path, utf8 ? new UTF8Encoding(false) : Encoding.GetEncoding(1252));
            var header = reader.ReadLine();
            if (header == null) return (null, "");
            var names = SplitCsvLine(header, delim);
            var n = names.Count;
            var rows = new long[n];
            var matching = new long[n];
            var max1 = new int[n];
            var max2 = new int[n];
            string? line;
            long lines = 0;
            while ((line = reader.ReadLine()) != null && lines++ < 3_000_000)
            {
                // A value with a newline inside quotes would split a row in two;
                // such a line has the wrong number of fields and is skipped, which
                // loses nothing but a sample.
                var cells = SplitCsvLine(line, delim);
                if (cells.Count != n) continue;
                for (var i = 0; i < n; i++)
                {
                    var v = cells[i].Trim();
                    if (v.Length == 0) continue;
                    rows[i]++;
                    if (!SlashDate.IsMatch(v)) continue;
                    matching[i]++;
                    var parts = v.Split('/');
                    var p2 = parts[1];
                    if (int.TryParse(parts[0], out var a) && a > max1[i]) max1[i] = a;
                    if (int.TryParse(p2, out var b) && b > max2[i]) max2[i] = b;
                }
            }

            var types = new object[n];
            var notes = new List<string>();
            var any = false;
            for (var i = 0; i < n; i++)
            {
                types[i] = 1; // xlGeneralFormat: Excel's own reading
                if (rows[i] == 0 || matching[i] != rows[i]) continue;
                if (max1[i] > 12 && max2[i] > 12) { types[i] = 2; any = true; notes.Add($"{names[i]} (mixed orders, left as text)"); }
                else if (max1[i] > 12) { types[i] = 4; any = true; notes.Add($"{names[i]} read day/month/year"); }
                else if (max2[i] > 12) { types[i] = 3; any = true; notes.Add($"{names[i]} read month/day/year"); }
                else { types[i] = 2; any = true; notes.Add($"{names[i]} (dates like {FirstSlashSample(path, delim, utf8, i)} that could be day/month or month/day, left as text so no order is guessed)"); }
            }
            if (!any) return (null, "");
            return (types, $" (date column(s): {string.Join("; ", notes)})");
        }

        private static string FirstSlashSample(string path, char delim, bool utf8, int column)
        {
            using var reader = new StreamReader(path, utf8 ? new UTF8Encoding(false) : Encoding.GetEncoding(1252));
            reader.ReadLine();
            string? line;
            while ((line = reader.ReadLine()) != null)
            {
                var cells = SplitCsvLine(line, delim);
                if (column < cells.Count && SlashDate.IsMatch(cells[column].Trim())) return cells[column].Trim();
            }
            return "1/2/2023";
        }

        /// <summary>One CSV line into its fields, honouring double quotes.</summary>
        private static List<string> SplitCsvLine(string line, char delim)
        {
            var cells = new List<string>();
            var sb = new StringBuilder();
            var quoted = false;
            for (var i = 0; i < line.Length; i++)
            {
                var c = line[i];
                if (quoted)
                {
                    if (c == '"' && i + 1 < line.Length && line[i + 1] == '"') { sb.Append('"'); i++; }
                    else if (c == '"') quoted = false;
                    else sb.Append(c);
                }
                else if (c == '"') quoted = true;
                else if (c == delim) { cells.Add(sb.ToString()); sb.Clear(); }
                else sb.Append(c);
            }
            cells.Add(sb.ToString());
            return cells;
        }
    }
}
