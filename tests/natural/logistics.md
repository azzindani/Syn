# A trucking company's database, 50 turns

Files: the fourteen CSVs in `Logistics_Operations_Database.zip` (a synthetic
trucking company's database, January 2022 to December 2024), extracted into
the chat's workspace folder. The script uses ten of them, one or two at a
time as the questions need them: `loads` (85,410 rows), `trips` (85,410),
`customers` (200), `drivers` (150), `routes` (58), `facilities` (50),
`delivery_events` (170,820), `fuel_purchases` (196,442), `maintenance_records`
(2,920) and `safety_incidents` (170), plus `trucks` (120). `DATABASE_SCHEMA.txt`
in the zip draws the keys. Unlike the other natural scripts this one is many
tables that join, so what it tests is whether a model can follow a key from
one file to another, correctly, in Excel. One conversation, one prompt per
message, in order.

What a careful analyst finds (checked against the files):

- **Keys that join:** `loads` to `customers` and `routes`; `trips` to `loads`
  (exactly one trip per load, 85,410 each), `drivers`, `trucks` and
  `trailers`; `delivery_events` to `trips` and `facilities` (two events per
  trip, a pickup and a delivery); `fuel_purchases` to `trips`;
  `maintenance_records` to `trucks`; `safety_incidents` to `trips`.
- **Money:** total `revenue` is 262,525,800.29, but customers are also
  charged `fuel_surcharge` and `accessorial_charges`; all three together are
  298,621,428.94. Revenue is nearly flat by year (87.9M, 87.0M, 87.7M).
- **Holes in the keys:** 1,714 trips have no driver, 1,672 no truck and 1,680
  no trailer (4,952 trips lack at least one, 37 lack both driver and truck).
  So anything summed per driver or per truck falls short of the total by
  about 2%: the per-driver monthly figures add up to 83,696 trips, not
  85,410. 3,880 fuel purchases have no truck and 3,988 no driver.
- **Customer names are not keys:** 200 customers share 107 names, so
  grouping by name merges different accounts. 32 customers are Inactive yet
  have 13,809 loads, and the biggest by revenue is one of them.
- **The on-time flag disagrees with the timestamps:** 55.7% of events are
  flagged on time (pickups 66.7%, deliveries 44.6%), but only 33% of events
  have an actual time at or before the scheduled one; the flag and the
  clock disagree on 56,866 events.
- **Fuel does not reconcile:** the purchases add up to 24.5 million gallons
  and the trips say they used 18.9 million. Purchases run to 2 January 2025,
  two days past the last trip, and the cities and states in the fuel,
  incident and event files do not belong together ("Denver, TX",
  "Charlotte, TN").
- **Utilisation does not stop at 100%:** `utilization_rate` in the truck
  monthly metrics goes up to 1.484.
- Every trip and load is Completed and every trip used an Active truck:
  nothing is cancelled, so a status filter finds nothing to remove.

## Excel

1. Open loads.csv, trips.csv, drivers.csv and customers.csv.
2. What's in each of these, and how do they connect to each other?
3. How many loads and how many trips are there, and what period do they cover?
4. Put all four into one workbook called Logistics.xlsx, one sheet each, and keep working in that.
5. Freeze the top row and make the headers bold on every sheet.
6. What's the total revenue across all loads?
7. Is that everything the customers were charged? Look at the other money columns and give me the total charged.
8. Add a Total Charged column to the loads sheet.
9. Show revenue by year in a small table on a new sheet called Yearly.
10. Who are the five biggest customers by revenue? I want names, not codes.
11. Add the customer's name and type to each load.
12. Some customer names look like they appear more than once. Are those the same customer?
13. How many loads were for customers whose account is inactive? Is that odd?
14. Open routes.csv and add it to the workbook. Then add the origin and destination to each load.
15. Which five lanes make the most revenue per mile, and which five the least? Put them on a sheet called Lanes.
16. Add the driver's name to each trip.
17. Some trips seem to have no driver. How many, and do they also have no truck?
18. Who are the ten drivers with the most revenue? Names please.
19. Add a month column to the loads and make a line chart of revenue by month, titled "Revenue by month".
20. Make a pivot of revenue with booking type down the side and customer type across.
21. Add a slicer for the load type.
22. Which booking type earns the most per load, and which the most per mile?
23. Open delivery_events.csv. What share of pickups and what share of deliveries were on time?
24. Does the on-time flag agree with the scheduled and actual times? Check.
25. Open facilities.csv. Which ten facilities have the longest average detention? Show the city too.
26. Open fuel_purchases.csv. How many gallons were bought in total, and how many do the trips say they used?
27. Those don't match. Look at the fuel data and tell me what you find.
28. Average price per gallon by year, with a chart.
29. Open maintenance_records.csv and trucks.csv. Which ten trucks cost the most to maintain? Show make and model year.
30. Which truck make costs the most to maintain per mile driven?
31. Open safety_incidents.csv. Incidents and total claims by type, and which drivers have the most incidents?
32. Build a Dashboard sheet with the revenue chart, the fuel price chart, and the total revenue, number of loads and on-time rate as big numbers. Make it fit on one screen.
33. Put the Dashboard first, then Yearly and Lanes, then the data sheets.
34. Hide the delivery events sheet. Actually, show it again. Then save.

## Word

35. Make a new Word document called "Fleet Performance 2022 to 2024".
36. Start with one paragraph on what the business looks like: loads, revenue, customers and drivers.
37. Add a section on revenue by year with the table and the revenue chart.
38. Add a section on lanes: the best and the worst per mile.
39. Add a section on service: on-time performance, and a plain note about the flag not matching the times.
40. Add a section on costs: fuel and maintenance.
41. Rewrite the first paragraph for a board member who has never seen the data, under 120 words.
42. Put the title in the header and page numbers in the footer, and save it.

## PowerPoint

43. Create a new PowerPoint deck for a ten-minute fleet review.
44. A title slide, then one slide per section of the report.
45. Use the charts on the slides instead of tables where you can.
46. Add a slide titled "What this data can't tell us", with three bullets.
47. Speaker notes on every slide, under 60 words each.
48. Remove the maintenance slide; it's too detailed for this audience.
49. Add a final slide with the one number to remember.
50. Save the deck.
