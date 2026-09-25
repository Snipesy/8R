package com.example;

public class Main {
    enum Color { RED, GREEN, BLUE }

    interface Shape { double area(); }

    static final class Circle implements Shape {
        private final double radius;
        Circle(double radius) { this.radius = radius; }
        public double area() { return Math.PI * radius * radius; }
    }

    static final class Square implements Shape {
        private final double side;
        Square(double side) { this.side = side; }
        public double area() { return side * side; }
    }

    private static int helper(int x) {
        return x * 2 + 1;
    }

    static String describe(Shape s) {
        return "area=" + s.area();
    }

    public static void main(String[] args) {
        Shape[] shapes = { new Circle(1.0), new Square(2.0) };
        for (Shape s : shapes) System.out.println(describe(s));
        for (Color c : Color.values()) System.out.println(c.name() + helper(c.ordinal()));
        Runnable r = () -> System.out.println("lambda " + args.length);
        r.run();
    }
}
