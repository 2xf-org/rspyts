"""
Smoke tests proving shared type identity across bridged crates.

The app crate bridges functions over ``Point`` and ``Axis`` without
defining them; ``[python.imports]`` maps their origin crate to the
``shared_types`` package. These tests pin the property that makes that
worthwhile: both packages hand out the *same* classes, so instances flow
between them without conversion.

Notes:
    Requires ``rspyts build --config examples/multi-crate/app/rspyts.toml``;
    the native library is staged beside the generated package.
"""

import shared_types

import multi_crate_app


class TestSharedIdentity:
    def test_point_is_the_same_class_in_both_packages(self):
        assert multi_crate_app.Point is shared_types.Point
        assert multi_crate_app.Axis is shared_types.Axis
        assert multi_crate_app.generated.models.Point is shared_types.generated.models.Point

    def test_translate_accepts_and_returns_the_shared_point(self):
        point = shared_types.Point(x=1.0, y=2.0)
        moved = multi_crate_app.translate(point, 3.0, 4.0)
        assert isinstance(moved, shared_types.Point)
        assert (moved.x, moved.y) == (4.0, 6.0)

    def test_mirror_accepts_the_shared_enum(self):
        point = shared_types.Point(x=1.0, y=2.0)
        flipped = multi_crate_app.mirror(point, shared_types.Axis.VERTICAL)
        assert isinstance(flipped, shared_types.Point)
        assert (flipped.x, flipped.y) == (-1.0, 2.0)

    def test_results_round_trip_back_into_app_functions(self):
        point = shared_types.Point(x=2.0, y=-3.0)
        flipped = multi_crate_app.mirror(point, multi_crate_app.Axis.HORIZONTAL)
        there_and_back = multi_crate_app.translate(flipped, 0.0, -3.0)
        assert isinstance(there_and_back, multi_crate_app.Point)
        assert (there_and_back.x, there_and_back.y) == (2.0, 0.0)
