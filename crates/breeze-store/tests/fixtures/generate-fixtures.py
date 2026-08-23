"""Emit reference JSON exactly as Breeze Core writes it, from its real models."""
import sys, json
sys.path.insert(0, "/src")
from meow_ac.config.models import AppConfig, UnitConfig, DevicesDoc, DeviceRecord
from meow_ac.programs.models import (
    ProgramsDoc, Program, ScheduleEntry, CurveConfig, CurvePoint,
)
from meow_ac.timers.models import TimersDoc, Timer
from meow_ac.devices.schemas import ControlRequest

out = {}

out["config.json"] = AppConfig(
    api_key="0123456789abcdef0123456789abcdef",
    units=[
        UnitConfig(name="Lijeva Soba", ip="192.168.1.73", port=6444, id=153931628470980,
                   token="aa" * 64, key="bb" * 32),
        # A V1/V2 unit: token and key are null, not absent.
        UnitConfig(name="No Creds", ip="192.168.1.99", id=1),
    ],
)

out["devices.json"] = DevicesDoc(devices=[
    # v1 bearer record, non-expiring, never used.
    DeviceRecord(token_id="abc123", label="Monique", auth_version=1,
                 token_hash="c" * 64, created_at=1787488496.123456),
    # v2 Ed25519 record with every optional populated.
    DeviceRecord(token_id="def456", label="Erkondišn", auth_version=2,
                 public_key="ZGVhZGJlZWZkZWFkYmVlZmRlYWRiZWVmZGVhZGJlZWZkZWE",
                 created_at=1787000000.0, expires_at=1790000000.5,
                 last_used=1787488000.25),
])

out["programs.json"] = ProgramsDoc(programs=[
    Program(id="p1", name="Favourite", kind="favourite",
            favourite=ControlRequest(power_state=True, target_temperature=24.5, beep=None)),
    Program(id="p2", name="Weekday", kind="schedule", unit_ids=["153931628470980"],
            schedule=[ScheduleEntry(days=[0, 1, 2, 3, 4], time="07:30",
                                    settings=ControlRequest(power_state=True,
                                                            operational_mode="COOL"))]),
    Program(id="p3", name="Curve", kind="curve", enabled=False,
            curve=CurveConfig(operational_mode="HEAT", fan_speed=60,
                              points=[CurvePoint(time="00:00", temperature=20.0),
                                      CurvePoint(time="12:00", temperature=23.5)])),
])

out["timers.json"] = TimersDoc(timers=[
    Timer(id="t1", unit_ids=["153931628470980"], minutes=45,
          created_at="2026-08-23T21:00:00", fires_at="2026-08-23T21:45:00",
          settings=ControlRequest(power_state=False), label="bedtime"),
])

for name, doc in out.items():
    text = doc.model_dump_json(indent=2)
    with open(f"/fixtures/{name}", "w", encoding="utf-8", newline="") as f:
        f.write(text)
    print(f"  {name}: {len(text)} bytes, no trailing newline={not text.endswith(chr(10))}")
