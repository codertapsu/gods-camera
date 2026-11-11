"""Replace reflect-mode Pad nodes with Slice and Concat nodes.

Reflect padding by `p` pixels is the same as copying rows `p..1` above the
image and rows `H-2..H-1-p` below it, then the same for columns. Slices and a
concatenation express that with operations every Core ML compute unit runs.
The output is numerically identical to the input model.

Why it matters, measured on an M4 Pro with ONNX Runtime 1.31.1 (see
docs/phase-0-results.md): the GPU path handles reflect padding by itself, so
this rewrite changes nothing there. The Neural Engine path does not. A
half-precision `inswapper_128` with reflect padding takes about 126 ms on the
Neural Engine, and about 37 ms after this rewrite.

Usage: uv run python rewrite_reflect_pad.py <in.onnx> <out.onnx>
"""
import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper


def read_pads(node, initializers, constants):
    attr = next((a for a in node.attribute if a.name == "pads"), None)
    if attr is not None:
        return list(attr.ints)
    if len(node.input) > 1:
        name = node.input[1]
        if name in initializers:
            return numpy_helper.to_array(initializers[name]).tolist()
        if name in constants:
            tensor = next(a.t for a in constants[name].attribute if a.name == "value")
            return numpy_helper.to_array(tensor).tolist()
    return None


def rewrite(model: onnx.ModelProto) -> int:
    graph = model.graph
    initializers = {i.name: i for i in graph.initializer}
    constants = {n.output[0]: n for n in graph.node if n.op_type == "Constant"}
    new_inits = {}

    def const(values):
        name = "rp_i64_" + "_".join(str(v).replace("-", "m") for v in values)
        if name not in new_inits and name not in initializers:
            new_inits[name] = numpy_helper.from_array(np.asarray(values, dtype=np.int64), name)
        return name

    def row(prefix, source, index_start, index_end, axis):
        """One row or column of `source`, selected by a single positive-step slice."""
        out = f"{prefix}_s{index_start}".replace("-", "m")
        node = helper.make_node(
            "Slice", [source, const([index_start]), const([index_end]), const([axis])], [out], name=out
        )
        return node, out

    def pad_axis(prefix, source, before, after, axis, final_output=None):
        if before == 0 and after == 0:
            return [], source
        nodes, parts = [], []
        for i in range(before, 0, -1):  # rows before..1, mirrored about row 0
            n, out = row(f"{prefix}_a{axis}_b", source, i, i + 1, axis)
            nodes.append(n)
            parts.append(out)
        parts.append(source)
        for i in range(1, after + 1):  # rows H-2 down to H-1-after, mirrored about row H-1
            n, out = row(f"{prefix}_a{axis}_e", source, -(i + 1), -i, axis)
            nodes.append(n)
            parts.append(out)
        out = final_output or f"{prefix}_a{axis}_cat"
        nodes.append(helper.make_node("Concat", parts, [out], name=f"{prefix}_a{axis}_concat", axis=axis))
        return nodes, out

    rewritten, nodes = 0, []
    for node in graph.node:
        mode = next((a.s.decode() for a in node.attribute if a.name == "mode"), "constant")
        pads = read_pads(node, initializers, constants) if node.op_type == "Pad" else None
        # Only 4-D reflect pads over height and width are handled; anything else is kept.
        if (
            node.op_type != "Pad"
            or mode != "reflect"
            or pads is None
            or len(pads) != 8
            or any(pads[i] != 0 for i in (0, 1, 4, 5))
        ):
            nodes.append(node)
            continue
        top, left, bottom, right = pads[2], pads[3], pads[6], pads[7]
        prefix = f"rp{rewritten}"
        needs_width = left or right
        h_nodes, h_out = pad_axis(prefix, node.input[0], top, bottom, 2, None if needs_width else node.output[0])
        w_nodes, _ = pad_axis(prefix, h_out, left, right, 3, node.output[0])
        if not h_nodes and not w_nodes:
            nodes.append(helper.make_node("Identity", [node.input[0]], [node.output[0]], name=f"{prefix}_id"))
        nodes.extend(h_nodes + w_nodes)
        rewritten += 1

    del graph.node[:]
    graph.node.extend(nodes)
    graph.initializer.extend(new_inits.values())
    return rewritten


def main() -> None:
    src, dst = sys.argv[1], sys.argv[2]
    model = onnx.load(src)
    count = rewrite(model)
    onnx.checker.check_model(model)
    onnx.save(model, dst)
    remaining = sum(1 for n in model.graph.node if n.op_type == "Pad")
    print(f"rewrote {count} reflect Pad nodes; {remaining} Pad nodes remain; wrote {dst}")


if __name__ == "__main__":
    main()
