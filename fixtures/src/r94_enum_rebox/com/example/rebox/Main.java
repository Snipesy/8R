package com.example.rebox;

enum Mode { OFF, LOW, HIGH, TURBO }

enum Dir { NORTH, EAST, SOUTH, WEST }

public class Main {
    public static void main(String[] args) {
        int n = args.length;
        String[] inputs = { "LOW", "TURBO", "OFF", n > 7 ? "BOGUS" : "HIGH" };
        StringBuilder out = new StringBuilder();
        for (String s : inputs) {
            Mode m = Mode.valueOf(s);
            Mode maybe = (m.ordinal() + n) % 3 == 0 ? null : m;
            out.append(m.name()).append(':').append(m.ordinal());
            switch (m) {
                case OFF: out.append(" off"); break;
                case LOW: out.append(" low"); break;
                default: out.append(" fast");
            }
            out.append(m == Mode.TURBO ? " T" : " -");
            out.append(maybe == null ? " null" : " " + maybe.name());
            out.append(' ').append(m.compareTo(Mode.HIGH));
            out.append('\n');
        }
        Dir d = Dir.values()[(n + 1) % 4];
        for (int i = 0; i < 5; i++) {
            out.append(d.name()).append(d == Dir.WEST ? "!" : ".");
            d = Dir.values()[(d.ordinal() + 1) % Dir.values().length];
        }
        out.append('\n').append(Dir.valueOf("SOUTH").ordinal()).append(Mode.values().length);
        System.out.println(out);
    }
}
