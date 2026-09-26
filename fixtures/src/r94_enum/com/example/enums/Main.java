package com.example.enums;

// Enum unboxing (docs/sources/r8-desugar.md §4.3): switches, name/valueOf/ordinal/hashCode/
// compareTo/values, enums with fields and constant-specific bodies.
enum Color { RED, GREEN, BLUE }

enum Planet {
    MERCURY(3.3), VENUS(4.8), EARTH(5.9), MARS(6.4);

    final double mass;

    Planet(double mass) { this.mass = mass; }
}

enum Op {
    ADD {
        int apply(int a, int b) { return a + b; }
    },
    MUL {
        int apply(int a, int b) { return a * b; }
    },
    SUB {
        int apply(int a, int b) { return a - b; }
    };

    abstract int apply(int a, int b);
}

enum Level { LOW, MEDIUM, HIGH, CRITICAL }

public class Main {
    static String describe(Color c) {
        switch (c) {
            case RED: return "warm";
            case GREEN: return "natural";
            default: return "cool";
        }
    }

    static int severity(Level l) {
        switch (l) {
            case LOW: return 1;
            case MEDIUM: return 5;
            case HIGH: return 10;
            default: return 100;
        }
    }

    public static void main(String[] args) {
        int n = args.length;
        Color c = Color.values()[n % 3];
        System.out.println(c.name() + " " + c.ordinal() + " " + describe(c));
        System.out.println(Color.valueOf(n > 5 ? "BLUE" : "GREEN").ordinal());
        Planet p = Planet.values()[(n + 2) % 4];
        System.out.println(p + " " + p.mass + " " + p.compareTo(Planet.EARTH));
        for (Op op : Op.values()) System.out.println(op.name() + "=" + op.apply(n + 6, 3));
        Level l = Level.values()[(n + 3) % 4];
        System.out.println(severity(l) + " " + (l == Level.CRITICAL) + " " + (l.hashCode() == l.hashCode()));
        Level none = n > 100 ? Level.LOW : null;
        System.out.println(none == null ? "null level" : none.name());
    }
}
