// A range that runs past the end of the sheet.
//
// `A1:A2000000` is not a range Excel has: a sheet ends at row 1,048,576 and
// column XFD. Asked for one, Excel answers 0x800A03EC with no words, and a
// model that had counted rows from a CSV and guessed high got nothing it could
// act on. The refusal says where the sheet ends and where its data does.

using System;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;

namespace Syn.Sidecar
{
    internal static partial class Program
    {
        private const int LastRow = 1_048_576, LastColumn = 16_384;

        /// <summary>ws.Range[addr], with a plain refusal when the address
        /// reaches past the last row or column.</summary>
        private static dynamic RangeOf(dynamic ws, string addr)
        {
            try { return ws.Range[addr]; }
            catch (COMException)
            {
                var past = PastTheEnd(addr);
                if (past.Length == 0) throw;
                string used = "";
                try { used = (string)ws.UsedRange.Address(false, false); } catch (Exception) { }
                throw new InvalidOperationException(
                    $"{(string)ws.Name}!{addr}: {past}. A sheet ends at row {LastRow.ToString("N0", CultureInfo.InvariantCulture)} and column XFD" +
                    (used.Length > 0 ? $"; the data on {(string)ws.Name} ends at {used}" : ""));
            }
        }

        /// <summary>What is wrong with an A1 address that Excel refused, when
        /// it is a row or column past the end; empty otherwise.</summary>
        private static string PastTheEnd(string addr)
        {
            foreach (var part in addr.Split(':', ','))
            {
                var m = Regex.Match(part.Trim(), @"^\$?([A-Za-z]*)\$?(\d*)$");
                if (!m.Success) continue;
                if (m.Groups[2].Length > 0 && long.TryParse(m.Groups[2].Value, out var row) && row > LastRow)
                    return $"row {row.ToString("N0", CultureInfo.InvariantCulture)} is past the last row";
                if (m.Groups[1].Length > 0)
                {
                    long col = 0;
                    foreach (var ch in m.Groups[1].Value.ToUpperInvariant()) col = col * 26 + (ch - 'A' + 1);
                    if (col > LastColumn) return $"column {m.Groups[1].Value.ToUpperInvariant()} is past the last column";
                }
            }
            return "";
        }
    }
}
