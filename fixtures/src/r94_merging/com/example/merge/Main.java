package com.example.merge;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;
import java.util.function.IntSupplier;

// Horizontal class merging (docs/sources/r8-desugar.md §4.1): same-shaped siblings with
// non-trivial virtuals get a classId field and switch dispatch; lambdas with the same capture
// shape are merged into lambda groups. Nothing prints class names (8R renames them).
interface Shape {
    double area();

    String label();
}

final class Circle implements Shape {
    final double r;

    Circle(double r) { this.r = r; }

    public double area() { return Math.PI * r * r; }

    public String label() { return "circle:" + r; }
}

final class Square implements Shape {
    final double s;

    Square(double s) { this.s = s; }

    public double area() { return s * s; }

    public String label() { return "square:" + s; }
}

final class Triangle implements Shape {
    final double b;

    Triangle(double b) { this.b = b; }

    public double area() { return b * b / 2; }

    public String label() { return "triangle:" + b; }
}

abstract class Base {
    int v;

    Base(int v) { this.v = v; }

    int f() { return v + 100; }
}

final class Doubler extends Base {
    Doubler(int v) { super(v); }

    int f() { return v * 2; }
}

final class Incrementer extends Base {
    Incrementer(int v) { super(v); }

    int f() { return super.f() + 1; }
}

interface One { int one(); }

interface Two { int two(); }

final class HasOne implements One {
    final int a;

    HasOne(int a) { this.a = a; }

    public int one() { return a + 1; }
}

final class HasTwo implements Two {
    final int a;

    HasTwo(int a) { this.a = a; }

    public int two() { return a * 3; }
}

public class Main {
    static List<Function<Integer, Integer>> functions(int k, String tag) {
        List<Function<Integer, Integer>> fs = new ArrayList<>();
        fs.add(x -> x + k);
        fs.add(x -> x * k);
        fs.add(x -> x - k);
        fs.add(x -> tag.length() + x);
        fs.add(x -> x ^ k);
        fs.add(x -> Integer.rotateLeft(x, k));
        return fs;
    }

    static IntSupplier[] suppliers(int n) {
        return new IntSupplier[] { () -> n + 1, () -> n * n, () -> n - 7, () -> Integer.bitCount(n) };
    }

    public static void main(String[] args) {
        int n = args.length + 3;
        Shape[] shapes = { new Circle(n), new Square(n + 1), new Triangle(n + 2) };
        for (Shape s : shapes) System.out.println(s.label() + " " + s.area());
        Base[] bases = { new Doubler(n), new Incrementer(n * 2) };
        for (Base b : bases) System.out.println(b.f());
        One one = new HasOne(n);
        Two two = new HasTwo(n);
        System.out.println(one.one() + two.two());
        for (Function<Integer, Integer> f : functions(n, "tag" + n)) System.out.println(f.apply(n * 5));
        for (IntSupplier s : suppliers(n)) System.out.println(s.getAsInt());
        Runnable r = () -> System.out.println("ran " + args.length);
        r.run();
    }
}
