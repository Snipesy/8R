package com.example.desugar;

import java.util.Objects;

interface Shape {
    int sides();

    default int doubled() {
        int s = sides();
        return s * 2 + s / 3 - Integer.hashCode(s);
    }

    default String label(String prefix) {
        return prefix + ":" + sides() + "/" + doubled();
    }

    static int total(Shape a, Shape b) {
        return a.sides() * 31 + b.sides() * 17 + Long.hashCode(a.sides());
    }
}

final class Tri implements Shape {
    public int sides() { return 3; }
}

final class Quad implements Shape {
    public int sides() { return 4; }
}

final class Pent implements Shape {
    public int sides() { return 5; }
    public int doubled() { return 11; }
}

public class Main {
    static int mix(long v, int w) {
        return Long.hashCode(v) ^ Integer.hashCode(w) + Math.floorMod(w, 7);
    }

    static int mix2(long v, int w) {
        return Long.hashCode(v * 3) + Math.floorMod(v, 5L) > 0 ? Math.addExact(w, 1) : Math.floorDiv(w, 3);
    }

    static String req(Object o, String what) {
        return Objects.requireNonNull(o, what).toString() + Objects.hashCode(o) + Boolean.hashCode(o == null);
    }

    public static void main(String[] args) {
        Shape[] shapes = { new Tri(), new Quad(), new Pent() };
        StringBuilder out = new StringBuilder();
        for (Shape s : shapes) {
            out.append(s.label("s")).append(' ').append(s.doubled()).append('\n');
        }
        out.append(Shape.total(shapes[0], shapes[1])).append(Shape.total(shapes[1], shapes[2])).append('\n');
        for (int i = 0; i < 6; i++) {
            out.append(mix(i * 1000003L, args.length + i)).append(',').append(mix2(i, i - 3)).append(';');
        }
        out.append('\n').append(req("x", "a")).append(req(args.length, "b")).append(Long.hashCode(args.length)).append(Double.hashCode(1.5));
        System.out.println(out);
    }
}
