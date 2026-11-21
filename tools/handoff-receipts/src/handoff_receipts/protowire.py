"""Small protobuf wire reader; no descriptors, recursive Any, or text guessing."""
from dataclasses import dataclass


@dataclass(frozen=True)
class Field:
    number: int
    wire: int
    value: object
    start: int
    end: int
    value_start: int


def fields(data, offset=0):
    if not isinstance(data, bytes):
        raise ValueError('protobuf cell must be bytes')
    position = 0

    def varint():
        nonlocal position
        value = 0
        for shift in range(0, 70, 7):
            if position >= len(data):
                raise ValueError('truncated protobuf varint')
            byte = data[position]
            position += 1
            if shift == 63 and byte > 1:
                raise ValueError('protobuf varint overflow')
            value |= (byte & 127) << shift
            if byte < 128:
                return value
        raise ValueError('protobuf varint overflow')

    result = []
    while position < len(data):
        start = position
        tag = varint()
        number, wire = tag >> 3, tag & 7
        if not 0 < number < 2 ** 29:
            raise ValueError('invalid protobuf field number')
        value_start = position
        if wire == 0:
            value = varint()
        elif wire in (1, 2, 5):
            length = varint() if wire == 2 else {1: 8, 5: 4}[wire]
            value_start = position
            end = position + length
            if end > len(data):
                raise ValueError('truncated protobuf field')
            value = data[position:end]
            position = end
        else:
            raise ValueError('unsupported protobuf wire type')
        result.append(Field(number, wire, value, offset + start, offset + position, offset + value_start))
    return result


def one(items, number, wire, required=True):
    matches = [f for f in items if f.number == number]
    if not matches and not required:
        return None
    if len(matches) != 1 or matches[0].wire != wire:
        raise ValueError('missing, repeated, or mistyped protobuf field')
    return matches[0]


def nested(field):
    if field.wire != 2:
        raise ValueError('protobuf message requires length-delimited field')
    return fields(field.value, field.value_start)
