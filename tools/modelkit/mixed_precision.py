"""Write a copy of a model in which some operations run in half precision and
the rest in full precision.

Why: on a graphics card, half precision makes the swap models about three
times faster. But through DirectML a wholly half-precision `inswapper_128`
gives a wrong picture. Its normalisation is spelt out as a mean, a
difference, a square, another mean, a square root and a division, and the
squares and means lose too much in 16 bits. Keeping those operations in full
precision and only the convolutions in half keeps the speed and the picture.
Always compare the result with check_swap.py before using it.

Two directions:

    # A full-precision model: everything to half precision except the named operations.
    uv run python mixed_precision.py half <in.onnx> <out.onnx> --keep ReduceMean Sub Mul Add Sqrt Div

    # A model stored in half precision: the named operations back to full precision.
    uv run python mixed_precision.py full <in.onnx> <out.onnx> --ops InstanceNormalization
"""
import argparse

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper
from onnxconverter_common import float16


def keep_outputs_full(graph) -> None:
    """Make every output of the graph full precision again.

    When the operation that produces an output is one of those kept in full
    precision, the converter still converts its result to half, and the output
    with it. Programs that read the output expect what the model had before.
    """
    producers = {name: node for node in graph.node for name in node.output}
    for output in graph.output:
        if output.type.tensor_type.elem_type != TensorProto.FLOAT16:
            continue
        node = producers.get(output.name)
        if node is not None and node.op_type == "Cast":
            # The conversion to half that the converter added: turn it around.
            for attribute in node.attribute:
                if attribute.name == "to":
                    attribute.i = TensorProto.FLOAT
        else:
            narrow = f"{output.name}__f16"
            for name_index, name in enumerate(node.output if node is not None else []):
                if name == output.name:
                    node.output[name_index] = narrow
            graph.node.append(helper.make_node("Cast", [narrow], [output.name], to=TensorProto.FLOAT, name=f"{output.name}__to_f32"))
        output.type.tensor_type.elem_type = TensorProto.FLOAT


def to_half(src: str, dst: str, keep: list[str]) -> None:
    # The converter's own list holds operations that cannot take half-precision
    # inputs at all, such as Resize with its scales. Naming a list replaces it,
    # so the two are joined.
    blocked = sorted(set(float16.DEFAULT_OP_BLOCK_LIST) | set(keep))
    model = float16.convert_float_to_float16(onnx.load(src), keep_io_types=True, op_block_list=blocked)
    keep_outputs_full(model.graph)
    onnx.checker.check_model(model)
    onnx.save(model, dst)
    casts = sum(1 for node in model.graph.node if node.op_type == "Cast")
    print(f"wrote {dst}: full precision kept for {keep}, {casts} conversions between the two")


def to_full(src: str, dst: str, ops: list[str]) -> None:
    """Run the named operations of a half-precision model in full precision."""
    model = onnx.load(src)
    graph = model.graph
    initializers = {init.name: init for init in graph.initializer}
    constants = {node.output[0]: node for node in graph.node if node.op_type == "Constant"}
    rewritten = []
    changed = 0
    for node in graph.node:
        if node.op_type not in ops:
            rewritten.append(node)
            continue
        changed += 1
        new_inputs = []
        for index, name in enumerate(node.input):
            if name in initializers and initializers[name].data_type == TensorProto.FLOAT16:
                wide = numpy_helper.from_array(numpy_helper.to_array(initializers[name]).astype(np.float32), f"{name}__f32")
                graph.initializer.append(wide)
                new_inputs.append(wide.name)
            elif name in constants or name not in initializers:
                # A value computed in the graph: convert it on the way in.
                cast = f"{node.name or node.output[0]}__in{index}_f32"
                rewritten.append(helper.make_node("Cast", [name], [cast], to=TensorProto.FLOAT, name=cast))
                new_inputs.append(cast)
            else:
                new_inputs.append(name)
        outputs = list(node.output)
        wide_outputs = [f"{name}__f32" for name in outputs]
        clone = helper.make_node(node.op_type, new_inputs, wide_outputs, name=node.name)
        clone.attribute.extend(node.attribute)
        rewritten.append(clone)
        for wide, narrow in zip(wide_outputs, outputs):
            rewritten.append(helper.make_node("Cast", [wide], [narrow], to=TensorProto.FLOAT16, name=f"{narrow}__to_f16"))
    del graph.node[:]
    graph.node.extend(rewritten)
    # Shapes recorded for the old values no longer hold for the new ones.
    del graph.value_info[:]
    onnx.checker.check_model(model)
    onnx.save(model, dst)
    print(f"wrote {dst}: {changed} {'/'.join(ops)} operations now run in full precision")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="direction", required=True)
    half = sub.add_parser("half", help="full-precision model to half, keeping some operations full")
    half.add_argument("src")
    half.add_argument("dst")
    half.add_argument("--keep", nargs="+", required=True, help="operation types that stay in full precision")
    full = sub.add_parser("full", help="half-precision model with some operations in full precision")
    full.add_argument("src")
    full.add_argument("dst")
    full.add_argument("--ops", nargs="+", required=True, help="operation types to run in full precision")
    args = parser.parse_args()
    if args.direction == "half":
        to_half(args.src, args.dst, args.keep)
    else:
        to_full(args.src, args.dst, args.ops)


if __name__ == "__main__":
    main()
