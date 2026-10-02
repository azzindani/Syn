# Test data

Real, published datasets, kept here so a fresh clone can run the tests that
use them. Each is used as it was downloaded; nothing is cleaned, because the
faults are part of what the tests check.

| File | Rows | Used by |
|---|---|---|
| `Solar_Energy_Production.csv` | 258,423 | `tests/capability/` (`setup.ps1` loads it from here) |
| `Hotel_Bookings_Demand.csv` | 119,390 | `tests/natural/hotels.md` |
| `US_Crude_Oil_Import.csv` | 483,053 | `tests/natural/oil.md` |
| `Global_Electricity_Production.csv` | 121,074 | `tests/natural/electricity.md` |
| `Electric_Vehicle_Population.zip` | 181,458 | `tests/natural/ev.md` |

The solar readings are City of Calgary open data, the electricity figures
are IEA monthly statistics, the oil imports are EIA data, and the vehicle
registrations are Washington State's. Each remains under its publisher's
terms.

The EV file is committed zipped. Extract `electric_vehicle_population_data.csv`
from it before use; the extracted copy is ignored by git.
