package com.example;

public class Outer {
    public static class InnerOne {
        public String hello() { return "one"; }
    }

    public static class InnerTwo {
        public String hello() { return "two"; }
    }

    public String both() {
        return new InnerOne().hello() + new InnerTwo().hello();
    }
}
